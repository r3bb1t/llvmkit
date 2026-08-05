//! On-the-fly SSA construction on top of the typed [`crate::IrBuilder`].
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
//! This module ships the typed variable/block vocabulary, the owned
//! [`SsaState`], construction, `create_block`/`declare_*`/`seal_block`,
//! the private Braun engine (`write_variable`, `read_variable_in`,
//! `add_phi_operands`, `try_remove_trivial_phi`, `emit_operandless_phi`,
//! `resolve`), and the full public lifecycle built on top of it:
//! `switch_to_block`/`finish` and `ins`/`current_block`/`def_*_var`/
//! `use_*_var`/the terminator family
//! `br`/`cond_br`/`switch`/`ret`/`ret_void`/`unreachable`.
//!
//! # The cursor model (0.0.4 cycle D)
//!
//! `SsaBuilder` is **one type**. Its insertion point is *data* — a
//! `cursor: Option<BlockId<..>>` field — not a type-state parameter that
//! changes the builder's type on `switch_to_block`. Every operation that
//! needs an insertion point reports [`IrError::SsaUnpositioned`] when the
//! cursor is empty, and every terminator clears it.
//!
//! This is a deliberate, *local* softening of Doctrine D1, and it is the
//! same trade the crate spells `_dyn` everywhere else: a static law is
//! rendered at runtime exactly where the tool's whole job is to be
//! dynamic. `SsaBuilder` **is** the dynamic-CFG tool — a step-driven
//! lifter discovers its blocks from the input it is decoding, holds the
//! builder across `&mut self` calls, and cannot thread a type-changing
//! value through a suspend/resume boundary without a take/put-back dance
//! (see `examples/lifter_session.rs`, which the old shape could not
//! express at all).
//!
//! What is emphatically **not** softened: [`crate::IrBuilder`]'s own
//! linear `BasicBlock` token and its terminator-consuming cursor stay
//! exactly as they were. A linear capability never becomes a `Copy` id —
//! that is why the phi `Open`/`Closed` marker was retired while the
//! terminator states were kept.
//!
//! [`SsaState`] carries the Braun bookkeeping and is an owned, `Send`,
//! `Clone`, lifetime-free value: a real lifter stores it in a struct
//! field and snapshots/restores the whole variable environment per
//! branch. The working builder is minted from `(&module, function,
//! &mut state)` and dropped again between steps.

use crate::Branded;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use super::basic_block::BasicBlock;
use super::block_state::Unterminated;
use super::constants::ConstantIntValue;
use super::float_kind::{FloatKind, IntoFloatValue, StaticFloatKind};
use super::function::FunctionValue;
use super::instruction::{Instruction, state::Attached};
use super::int_width::{IntWidth, IntoConstantInt, IntoIntValue, StaticIntWidth};
use super::ir_builder::constant_folder::ConstantFolder;
use super::ir_builder::folder::IrBuilderFolder;
use super::ir_builder::{IntoReturnValue, Positioned};
use super::marker::{Dyn, ReturnMarker};
use super::module::{Invariant, Module, ModuleBrand, ModuleRef, Unverified};
use super::r#type::TypeSlot;
use super::value::{
    FloatValue, IntValue, IntoPointerValue, IsValue, PointerValue, Typed, Value, ValueSlot,
};
use super::value_id::BlockId;
use super::{FloatType, IntType, IrError, IrResult, PointerType};

/// Folds either of `IntoConstantInt`'s two possible associated `Error`
/// types -- `Infallible` for exact-width lifts, [`IrError`] for
/// `IntDyn`-target lifts (see `int_width.rs`) -- down to [`IrResult`]
/// uniformly. Exists so [`SsaBuilder::switch`] can stay generic over
/// `W: IntWidth` for both static markers and `IntDyn` in one signature
/// rather than needing a copy per error shape.
///
/// A crate-wide `impl From<Infallible> for IrError` would let `?` do
/// this instead, but it also gives `IrError: From<E>` a second solution
/// (`E = Infallible`, alongside the reflexive `E = IrError`) everywhere
/// a `?`-chain's error type is inferred purely from an outer
/// `IrError: From<E>` constraint -- `examples/derived_struct_function.rs`'s
/// module-building block hits exactly that ambiguity. Converting on
/// the CONCRETE `Result<T, Infallible>` / `Result<T, IrError>` types
/// instead of adding an impl to `IrError` itself means this cannot
/// perturb inference anywhere else in the crate.
///
/// Public (not `pub(crate)`) because it appears in `switch`'s public
/// `where` clause; `#[doc(hidden)]` on the method since callers never
/// invoke it directly (mirrors [`function_signature::IntoCallArg`](
/// crate::function_signature::IntoCallArg)'s public-trait/hidden-method
/// split for the same "bound must be nameable, method is plumbing" shape).
pub trait IntoIrResult<T> {
    #[doc(hidden)]
    fn into_ir_result(self) -> IrResult<T>;
}
impl<T> IntoIrResult<T> for Result<T, core::convert::Infallible> {
    #[inline]
    fn into_ir_result(self) -> IrResult<T> {
        match self {
            Ok(v) => Ok(v),
            Err(never) => match never {},
        }
    }
}
impl<T> IntoIrResult<T> for IrResult<T> {
    #[inline]
    fn into_ir_result(self) -> IrResult<T> {
        self
    }
}

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
///
/// Lifetime-free since cycle D: the handle is a `Copy`, module-tagged id
/// exactly like [`crate::IntValueId`] &c, so it can be stored in a
/// lifter's own struct alongside the module and the [`SsaState`]. The
/// owning module is pinned by the *type* parameter `B`, not by a stored
/// `ModuleRef` — resolution happens at the def/use call sites, which
/// already hold the module.
pub struct IntVariable<W: IntWidth, B: ModuleBrand> {
    index: u32,
    owner: SsaBuilderId,
    ty: TypeSlot,
    _b: Invariant<B>,
    _w: core::marker::PhantomData<fn() -> W>,
}

impl<W: IntWidth, B: ModuleBrand> Clone for IntVariable<W, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<W: IntWidth, B: ModuleBrand> Copy for IntVariable<W, B> {}
impl<W: IntWidth, B: ModuleBrand> core::fmt::Debug for IntVariable<W, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("IntVariable")
            .field("index", &self.index)
            .field("owner", &self.owner)
            .finish()
    }
}
impl<W: IntWidth, B: ModuleBrand> PartialEq for IntVariable<W, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && self.owner == other.owner && self.ty == other.ty
    }
}
impl<W: IntWidth, B: ModuleBrand> Eq for IntVariable<W, B> {}
impl<W: IntWidth, B: ModuleBrand> core::hash::Hash for IntVariable<W, B> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.index.hash(h);
        self.owner.hash(h);
        self.ty.hash(h);
    }
}

impl<W: IntWidth, B: ModuleBrand> IntVariable<W, B> {
    /// The [`SsaBuilder`] that declared this variable. Foreign use (a
    /// handle from a different builder) is a typed runtime error at the
    /// def/use call sites.
    #[inline]
    pub fn owner(&self) -> SsaBuilderId {
        self.owner
    }
}

/// Typed SSA variable of float kind `K`. Lifetime-free for the same
/// reason as [`IntVariable`].
pub struct FloatVariable<K: FloatKind, B: ModuleBrand> {
    index: u32,
    owner: SsaBuilderId,
    ty: TypeSlot,
    _b: Invariant<B>,
    _k: core::marker::PhantomData<fn() -> K>,
}

impl<K: FloatKind, B: ModuleBrand> Clone for FloatVariable<K, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<K: FloatKind, B: ModuleBrand> Copy for FloatVariable<K, B> {}
impl<K: FloatKind, B: ModuleBrand> core::fmt::Debug for FloatVariable<K, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FloatVariable")
            .field("index", &self.index)
            .field("owner", &self.owner)
            .finish()
    }
}
impl<K: FloatKind, B: ModuleBrand> PartialEq for FloatVariable<K, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && self.owner == other.owner && self.ty == other.ty
    }
}
impl<K: FloatKind, B: ModuleBrand> Eq for FloatVariable<K, B> {}
impl<K: FloatKind, B: ModuleBrand> core::hash::Hash for FloatVariable<K, B> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.index.hash(h);
        self.owner.hash(h);
        self.ty.hash(h);
    }
}

impl<K: FloatKind, B: ModuleBrand> FloatVariable<K, B> {
    /// The [`SsaBuilder`] that declared this variable.
    #[inline]
    pub fn owner(&self) -> SsaBuilderId {
        self.owner
    }
}

/// Typed SSA variable of pointer category (any address space).
/// Lifetime-free for the same reason as [`IntVariable`].
pub struct PointerVariable<B: ModuleBrand> {
    index: u32,
    owner: SsaBuilderId,
    ty: TypeSlot,
    _b: Invariant<B>,
}

impl<B: ModuleBrand> Clone for PointerVariable<B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<B: ModuleBrand> Copy for PointerVariable<B> {}
impl<B: ModuleBrand> core::fmt::Debug for PointerVariable<B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PointerVariable")
            .field("index", &self.index)
            .field("owner", &self.owner)
            .finish()
    }
}
impl<B: ModuleBrand> PartialEq for PointerVariable<B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && self.owner == other.owner && self.ty == other.ty
    }
}
impl<B: ModuleBrand> Eq for PointerVariable<B> {}
impl<B: ModuleBrand> core::hash::Hash for PointerVariable<B> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.index.hash(h);
        self.owner.hash(h);
        self.ty.hash(h);
    }
}

impl<B: ModuleBrand> PointerVariable<B> {
    /// The [`SsaBuilder`] that declared this variable.
    #[inline]
    pub fn owner(&self) -> SsaBuilderId {
        self.owner
    }
}

/// Copyable reference to a block managed by an [`SsaBuilder`]. NOT an
/// insertion capability -- the linear `BasicBlock` handles stay inside the
/// `SsaBuilder`; this implements [`crate::IntoBasicBlockLabel`] as the
/// escape hatch for feeding a `br`/successor built through the plain
/// [`IrBuilder`] surface elsewhere.
///
/// Wraps the storable [`BlockId`] currency with the owning builder's identity,
/// so it is lifetime-free like the id it carries.
///
/// [`IrBuilder`]: crate::IrBuilder
pub struct SsaBlock<R: ReturnMarker, B: ModuleBrand> {
    id: BlockId<R, B>,
    owner: SsaBuilderId,
}

impl<R: ReturnMarker, B: ModuleBrand> Clone for SsaBlock<R, B> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<R: ReturnMarker, B: ModuleBrand> Copy for SsaBlock<R, B> {}
impl<R: ReturnMarker, B: ModuleBrand> core::fmt::Debug for SsaBlock<R, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SsaBlock")
            .field("id", &self.id)
            .field("owner", &self.owner)
            .finish()
    }
}
impl<R: ReturnMarker, B: ModuleBrand> PartialEq for SsaBlock<R, B> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        // `BlockId`'s hand-written `PartialEq` compares `tag`/`slot` only — it
        // deliberately does *not* bound `R: PartialEq` (which `ReturnMarker`
        // does not guarantee) — so this stays clear of the phantom markers,
        // exactly as `BasicBlock`'s own manual `PartialEq` does.
        self.id == other.id && self.owner == other.owner
    }
}
impl<R: ReturnMarker, B: ModuleBrand> Eq for SsaBlock<R, B> {}
impl<R: ReturnMarker, B: ModuleBrand> core::hash::Hash for SsaBlock<R, B> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.owner.hash(h);
    }
}

impl<R: ReturnMarker, B: ModuleBrand> SsaBlock<R, B> {
    /// The underlying storable [`BlockId`], usable anywhere a
    /// [`crate::IntoBasicBlockLabel`] source is accepted (e.g. a plain
    /// `IrBuilder::br` target).
    #[inline]
    pub fn id(&self) -> BlockId<R, B> {
        self.id
    }
}

// `IntoBasicBlockLabel` is sealed to `basic_block.rs` (its `Sealed`
// marker trait is a private submodule there), so `SsaBlock`'s impl lives
// alongside the other implementors in that file instead of here.

/// Diagnostic name for a block id: falls back to a slot-style
/// placeholder when the block was never given a textual name, mirroring
/// how the AsmWriter falls back to numbered slots.
fn block_name<'ctx, B: ModuleBrand + 'ctx>(
    module: ModuleRef<'ctx, B>,
    block_id: ValueSlot,
) -> String {
    let label_ty = module.module().label_type::<B>().as_type().id();
    let label = BasicBlock::<Dyn, Unterminated, B>::from_parts(block_id, module, label_ty).label();
    label
        .to_erased()
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

#[derive(Debug, Clone)]
struct VarData {
    ty: TypeSlot,
    category: VarCategory,
    name: String,
    poison_on_undef: bool,
}

/// The on-the-fly SSA construction state for one function: Braun's
/// `currentDef` / `incompletePhis` / `sealedBlocks` / `filledBlocks`
/// bookkeeping plus the declared-variable table.
///
/// **Owned, `Send`, `Clone`, lifetime-free.** It holds no borrow of the
/// module and no linear IR capability — only `ValueSlot`s and the
/// module brand `B` as a phantom tag — so it can live in a struct
/// field, cross a thread boundary alongside its [`Module`], and be
/// *snapshotted and restored* around a speculative branch. (A real
/// lifter saves its whole variable environment before a conditional and
/// restores it on the other arm; the C++ system this crate is modelled
/// on does exactly that.)
///
/// The state is inert on its own: mint a short-lived [`SsaBuilder`] from
/// `(&module, function, &mut state)` to author IR through it, and drop
/// the builder again between steps.
///
/// ```
/// use llvmkit_ir::{Linkage, Module, SsaBuilder, SsaState};
///
/// # fn main() -> Result<(), llvmkit_ir::IrError> {
/// let m = Module::dynamic("session");
/// let f = m
///     .add_typed_function::<(), (), _>("f", Linkage::External)?
///     .as_function();
///
/// // The state outlives every builder minted from it.
/// let mut state = SsaState::for_function(&m, m.view(f))?;
///
/// let entry = {
///     let mut b = SsaBuilder::for_function(&m, m.view(f), &mut state)?;
///     b.create_block("entry")
/// };
/// {
///     let mut b = SsaBuilder::for_function(&m, m.view(f), &mut state)?;
///     b.switch_to_block(entry)?;
///     b.ret_void()?;
///     b.finish()?;
/// }
/// assert!(format!("{m}").contains("ret void"));
/// # Ok(()) }
/// ```
#[derive(Branded)]
#[branded(Debug, Clone)]
pub struct SsaState<B: ModuleBrand> {
    /// The function this state is authoring. Every builder minted from it
    /// must name the same function ([`IrError::SsaForeignFunction`]).
    function: ValueSlot,
    /// Identity every [`SsaBlock`] / variable handle this state's builders
    /// hand out is stamped with.
    id: SsaBuilderId,
    vars: Vec<VarData>,
    /// Braun `currentDef`: `(block, var) -> definition value`.
    current_def: HashMap<(ValueSlot, u32), ValueSlot>,
    /// Trivial-phi forwarding (path-compressed on read).
    resolved: RefCell<HashMap<ValueSlot, ValueSlot>>,
    /// Recorded CFG edges, duplicates preserved (phi operand order).
    preds: HashMap<ValueSlot, Vec<ValueSlot>>,
    sealed: HashSet<ValueSlot>,
    /// Braun `filledBlocks`: blocks that have received their terminator.
    /// Populated by the terminator-building methods (`br`/`cond_br`/
    /// `switch`/`ret`/`ret_void`/`unreachable`); consulted by
    /// `switch_to_block` (reject repositioning into a filled block --
    /// `IrError::SsaBlockAlreadyFilled`) and `finish` (every created
    /// block must be filled -- `IrError::SsaUnfilledBlock`).
    filled: HashSet<ValueSlot>,
    /// Braun `incompletePhis`: `block -> [(var index, phi value)]`.
    incomplete_phis: HashMap<ValueSlot, Vec<(u32, ValueSlot)>>,
    /// Slots of the phis this layer created and still tracks. Cycle D
    /// stores the raw slot rather than a linear
    /// [`Instruction`](crate::Instruction) handle: keeping a linear
    /// capability here is what forced `SsaState` to carry a `'ctx`, and
    /// the RAUW/erase sites re-mint the handle from the slot at the
    /// moment they need it (`Instruction::from_parts`), which is the same
    /// crate-internal move `try_remove_trivial_phi`'s erase step already
    /// made.
    created_phis: HashSet<ValueSlot>,
    /// `phi -> declaring variable index`, populated alongside
    /// `created_phis` in `emit_operandless_phi` (the one place that
    /// KNOWS which variable a phi was created for). Lets
    /// `undefined_phi_replacement` key strict/poison off `vars[idx]`
    /// directly instead of re-deriving the variable from the phi's
    /// cached type -- see that method's doc comment (D10).
    phi_var: HashMap<ValueSlot, u32>,
    /// Deterministic iteration for a future `finish()`.
    block_order: Vec<ValueSlot>,
    _b: Invariant<B>,
}

impl<B: ModuleBrand> SsaState<B> {
    /// Begin on-the-fly SSA construction for `function`.
    ///
    /// Errors with [`IrError::SsaFunctionHasBlocks`] if `function`
    /// already has a body -- the layer must observe every CFG edge from
    /// birth, so grafting onto a partially-built function is rejected.
    /// This is the *only* place that check runs: re-minting a builder
    /// over an in-progress state must obviously not re-trip it.
    pub fn for_function<'ctx, R: ReturnMarker>(
        module: &'ctx Module<B, Unverified>,
        function: FunctionValue<'ctx, R, B>,
    ) -> IrResult<Self>
    where
        B: 'ctx,
    {
        if function.entry_block().is_some() {
            return Err(IrError::SsaFunctionHasBlocks);
        }
        Ok(Self {
            function: function.slot(),
            id: SsaBuilderId(module.next_ssa_builder_id()),
            vars: Vec::new(),
            current_def: HashMap::new(),
            resolved: RefCell::new(HashMap::new()),
            preds: HashMap::new(),
            sealed: HashSet::new(),
            filled: HashSet::new(),
            incomplete_phis: HashMap::new(),
            created_phis: HashSet::new(),
            phi_var: HashMap::new(),
            block_order: Vec::new(),
            _b: core::marker::PhantomData,
        })
    }

    /// The identity stamped onto every [`SsaBlock`] and declared variable
    /// handed out by a builder over this state. Two states never share
    /// one, so a handle from another session is refused
    /// ([`IrError::SsaForeignBlock`] / [`IrError::SsaForeignVariable`]) --
    /// including a handle from a *snapshot clone*, which deliberately
    /// keeps the original's id so a restored environment still accepts
    /// the blocks it was minted with.
    #[inline]
    pub fn id(&self) -> SsaBuilderId {
        self.id
    }

    /// Number of blocks created through this state so far.
    #[inline]
    pub fn block_count(&self) -> usize {
        self.block_order.len()
    }

    /// Number of variables declared through this state so far.
    #[inline]
    pub fn variable_count(&self) -> usize {
        self.vars.len()
    }
}

/// Cranelift-`FunctionBuilder`-style layer on top of the typed
/// [`IrBuilder`] implementing Braun et al.'s on-the-fly SSA construction
/// (sealed blocks, incomplete phis, trivial-phi elimination). See the
/// module docs for the algorithm citation and for why the insertion
/// point is *data* rather than a type-state parameter.
///
/// The builder is a short-lived working handle: it borrows the module
/// and its [`SsaState`], and holds nothing that cannot be rebuilt from
/// the two. Mint one, drive it, drop it; the session lives in the state.
///
/// [`IrBuilder`]: crate::IrBuilder
pub struct SsaBuilder<'s, 'ctx, B, F = ConstantFolder, R = Dyn>
where
    B: ModuleBrand,
    F: IrBuilderFolder<'ctx, B> + Clone,
    R: ReturnMarker,
{
    module: &'ctx Module<B, Unverified>,
    function: FunctionValue<'ctx, R, B>,
    folder: F,
    /// The cursor, as data: `None` = unpositioned. Every operation that
    /// needs an insertion point reports [`IrError::SsaUnpositioned`] when
    /// it is empty, and every terminator empties it.
    ///
    /// The payload is the positioned plain [`IrBuilder`] itself rather
    /// than a bare [`BlockId`], for one reason: it is what lets
    /// [`ins`](Self::ins) hand out a **borrow**. A borrowed positioned
    /// builder cannot reach the plain builder's `self`-consuming
    /// terminators, so an `ins()` caller structurally cannot terminate a
    /// block behind this layer's back and leave the Braun edge
    /// bookkeeping incomplete. The block id is recoverable from it at
    /// any time ([`current_block`](Self::current_block)), so nothing
    /// about "the position is data" is given up.
    ///
    /// [`IrBuilder`]: crate::IrBuilder
    cursor: Option<super::ir_builder::IrBuilder<'ctx, 'ctx, B, F, Positioned, R>>,
    state: &'s mut SsaState<B>,
}

impl<'s, 'ctx, B: ModuleBrand + 'ctx, R: ReturnMarker> SsaBuilder<'s, 'ctx, B, ConstantFolder, R> {
    /// Mint a working builder over `state` using the default
    /// [`ConstantFolder`].
    ///
    /// `function` must be the one `state` was opened for
    /// ([`SsaState::for_function`]), otherwise
    /// [`IrError::SsaForeignFunction`]. The builder starts unpositioned;
    /// call [`switch_to_block`](Self::switch_to_block) before emitting.
    pub fn for_function(
        module: &'ctx Module<B, Unverified>,
        function: FunctionValue<'ctx, R, B>,
        state: &'s mut SsaState<B>,
    ) -> IrResult<Self> {
        Self::with_folder_for_function(module, function, state, ConstantFolder)
    }
}

impl<'s, 'ctx, B, F, R> SsaBuilder<'s, 'ctx, B, F, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B> + Clone,
    R: ReturnMarker,
{
    /// [`for_function`](SsaBuilder::for_function) with a caller-supplied
    /// folder.
    pub fn with_folder_for_function(
        module: &'ctx Module<B, Unverified>,
        function: FunctionValue<'ctx, R, B>,
        state: &'s mut SsaState<B>,
        folder: F,
    ) -> IrResult<Self> {
        if state.function != function.slot() {
            return Err(IrError::SsaForeignFunction);
        }
        Ok(SsaBuilder {
            module,
            function,
            folder,
            cursor: None,
            state,
        })
    }
}

// --------------------------------------------------------------------------
// Session surface: create_block, variable declarations, seal_block
// --------------------------------------------------------------------------

impl<'s, 'ctx, B, F, R> SsaBuilder<'s, 'ctx, B, F, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B> + Clone,
    R: ReturnMarker,
{
    /// This session's per-module id. Exposed for diagnostics /
    /// cross-checking; ordinary callers do not need to inspect it.
    /// Same value as [`SsaState::id`], since the identity lives in the
    /// state rather than in the (re-mintable) builder.
    #[inline]
    pub fn id(&self) -> SsaBuilderId {
        self.state.id
    }

    /// Read-only access to the session state this builder is driving.
    #[inline]
    pub fn state(&self) -> &SsaState<B> {
        self.state
    }

    /// Append a block. The FIRST created block is the entry block and is
    /// auto-Braun-sealed: entry has no predecessors by definition
    /// (`Verifier::visitFunction`), so a later branch TO it errors with
    /// [`IrError::SsaBranchToSealedBlock`] once edge-recording lands.
    pub fn create_block<Name: Into<String>>(&mut self, name: Name) -> SsaBlock<R, B> {
        let block = self.function.append_basic_block(self.module, name);
        let id = block.id();
        let block_id = block.slot();
        if self.state.block_order.is_empty() {
            self.state.sealed.insert(block_id);
        }
        self.state.block_order.push(block_id);
        self.state.preds.entry(block_id).or_default();
        SsaBlock {
            id,
            owner: self.state.id,
        }
    }

    /// Declare a strict int variable: reading it on a def-less path is a
    /// typed error (D10).
    pub fn declare_int_var<W: StaticIntWidth, Name: Into<String>>(
        &mut self,
        name: Name,
    ) -> IntVariable<W, B> {
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
    ) -> IntVariable<W, B> {
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
    ) -> IntVariable<super::int_width::IntDyn, B> {
        self.declare_var_raw(ty.as_type().id(), name, VarCategory::Int, false)
            .into()
    }

    /// Poison twin of [`Self::declare_int_var_dyn`].
    pub fn declare_int_var_dyn_poison<Name: Into<String>>(
        &mut self,
        ty: IntType<'ctx, super::int_width::IntDyn, B>,
        name: Name,
    ) -> IntVariable<super::int_width::IntDyn, B> {
        self.declare_var_raw(ty.as_type().id(), name, VarCategory::Int, true)
            .into()
    }

    /// Declare a strict float variable.
    pub fn declare_float_var<K: StaticFloatKind, Name: Into<String>>(
        &mut self,
        name: Name,
    ) -> FloatVariable<K, B> {
        let ty = K::ir_type(self.module_ref()).as_type().id();
        self.declare_var_raw(ty, name, VarCategory::Float, false)
            .into()
    }

    /// Poison twin of [`Self::declare_float_var`].
    pub fn declare_float_var_poison<K: StaticFloatKind, Name: Into<String>>(
        &mut self,
        name: Name,
    ) -> FloatVariable<K, B> {
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
    ) -> FloatVariable<super::float_kind::FloatDyn, B> {
        self.declare_var_raw(ty.as_type().id(), name, VarCategory::Float, false)
            .into()
    }

    /// Poison twin of [`Self::declare_float_var_dyn`].
    pub fn declare_float_var_dyn_poison<Name: Into<String>>(
        &mut self,
        ty: FloatType<'ctx, super::float_kind::FloatDyn, B>,
        name: Name,
    ) -> FloatVariable<super::float_kind::FloatDyn, B> {
        self.declare_var_raw(ty.as_type().id(), name, VarCategory::Float, true)
            .into()
    }

    /// Declare a strict pointer variable in the default address space
    /// (addrspace 0).
    pub fn declare_pointer_var<Name: Into<String>>(&mut self, name: Name) -> PointerVariable<B> {
        let ty = self.module.ptr_type(0).as_type().id();
        self.declare_var_raw(ty, name, VarCategory::Pointer, false)
            .into()
    }

    /// Poison twin of [`Self::declare_pointer_var`].
    pub fn declare_pointer_var_poison<Name: Into<String>>(
        &mut self,
        name: Name,
    ) -> PointerVariable<B> {
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
    ) -> PointerVariable<B> {
        self.declare_var_raw(ty.as_type().id(), name, VarCategory::Pointer, false)
            .into()
    }

    /// Poison twin of [`Self::declare_pointer_var_in_addrspace`].
    pub fn declare_pointer_var_in_addrspace_poison<Name: Into<String>>(
        &mut self,
        ty: PointerType<'ctx, B>,
        name: Name,
    ) -> PointerVariable<B> {
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
        ty: TypeSlot,
        name: Name,
        category: VarCategory,
        poison_on_undef: bool,
    ) -> VarHandle<B> {
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
            owner: self.state.id,
            ty,
            _b: core::marker::PhantomData,
        }
    }

    #[inline]
    fn module_ref(&self) -> ModuleRef<'ctx, B> {
        ModuleRef::new(self.module.core_ref())
    }

    fn check_owner_block(&self, block: &SsaBlock<R, B>) -> IrResult<()> {
        if block.owner != self.state.id {
            return Err(IrError::SsaForeignBlock);
        }
        Ok(())
    }

    /// Sibling to [`Self::check_owner_block`] for variable handles: a
    /// declared variable used against a different `SsaBuilder` than the
    /// one that declared it is a typed runtime error. Takes just the
    /// owner id (rather than a whole variable handle) since
    /// `IntVariable`/`FloatVariable`/`PointerVariable` are three
    /// unrelated structs with no shared trait -- every def/use call site
    /// passes `var.owner`.
    fn check_owner_var(&self, owner: SsaBuilderId) -> IrResult<()> {
        if owner != self.state.id {
            return Err(IrError::SsaForeignVariable);
        }
        Ok(())
    }

    /// Braun `sealBlock`: the predecessor set is complete; complete this
    /// block's incomplete phis.
    pub fn seal_block(&mut self, block: SsaBlock<R, B>) -> IrResult<()> {
        self.check_owner_block(&block)?;
        let block_id = block.id.slot();
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

// --------------------------------------------------------------------------
// Cursor: switch_to_block, ins, current_block, finish
// --------------------------------------------------------------------------

impl<'s, 'ctx, B, F, R> SsaBuilder<'s, 'ctx, B, F, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B> + Clone,
    R: ReturnMarker,
{
    /// Move the cursor to the end of `block`.
    ///
    /// Unlike the pre-cycle-D shape this does not change the builder's
    /// *type* -- the position is data. "Terminate the block you are in
    /// before moving on" is therefore a runtime law here rather than a
    /// static one, enforced by the same [`IrError::SsaBlockAlreadyFilled`]
    /// this method has always raised for an already-terminated target,
    /// plus `finish`'s [`IrError::SsaUnfilledBlock`] sweep for a block
    /// abandoned half-built. See the module docs for why the SSA layer --
    /// and only the SSA layer -- makes that trade.
    pub fn switch_to_block(&mut self, block: SsaBlock<R, B>) -> IrResult<()> {
        self.check_owner_block(&block)?;
        let block_id = block.id.slot();
        if self.state.filled.contains(&block_id) {
            return Err(IrError::SsaBlockAlreadyFilled {
                block: block_name(self.module_ref(), block_id),
            });
        }
        // `position_at_end_dyn` re-derives the plain builder's linear
        // insertion token from the id and re-checks that the block is
        // still open, so the strict builder's own guarantee is enforced
        // at every reposition rather than assumed from a token this
        // layer had been hoarding.
        self.cursor = Some(
            super::ir_builder::IrBuilder::with_folder(self.module, self.folder.clone())
                .position_at_end_dyn(block.id)?,
        );
        Ok(())
    }

    /// `true` while the cursor names a block.
    #[inline]
    pub fn is_positioned(&self) -> bool {
        self.cursor.is_some()
    }

    /// Clear the cursor without emitting anything. The block keeps its
    /// (unfilled) status, so `finish` will still insist on a terminator
    /// for it.
    #[inline]
    pub fn clear_position(&mut self) {
        self.cursor = None;
    }

    /// Seal every remaining unsealed block (draining their incomplete
    /// phis via the private `add_phi_operands` engine step, exactly as
    /// [`Self::seal_block`] would), then require every created block to
    /// have been filled (received a terminator).
    ///
    /// Consuming `self` releases the borrow on the [`SsaState`]. A
    /// builder that is still positioned when `finish` runs is sitting in
    /// a block with no terminator, which is exactly the
    /// [`IrError::SsaUnfilledBlock`] case -- reported up front so the
    /// error names the block the caller was mid-way through rather than
    /// whichever unfilled block happens to come first in creation order.
    pub fn finish(mut self) -> IrResult<()> {
        if let Some(open) = self.cursor.as_ref().map(|b| b.insert_block().slot()) {
            return Err(IrError::SsaUnfilledBlock {
                block: block_name(self.module_ref(), open),
            });
        }
        for block_id in self.state.block_order.clone() {
            if !self.state.sealed.contains(&block_id) {
                let pending = self
                    .state
                    .incomplete_phis
                    .remove(&block_id)
                    .unwrap_or_default();
                self.state.sealed.insert(block_id);
                for (var, phi) in pending {
                    self.add_phi_operands(var, phi, block_id)?;
                }
            }
        }
        for block_id in &self.state.block_order {
            if !self.state.filled.contains(block_id) {
                return Err(IrError::SsaUnfilledBlock {
                    block: block_name(self.module_ref(), *block_id),
                });
            }
        }
        Ok(())
    }
}

// --------------------------------------------------------------------------
// Positioned surface: ins, current_block, def/use, terminators
// --------------------------------------------------------------------------

impl<'s, 'ctx, B, F, R> SsaBuilder<'s, 'ctx, B, F, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B> + Clone,
    R: ReturnMarker,
{
    /// The full existing typed instruction surface, cranelift-style:
    /// `b.ins()?.int_mul(a, b, "x")?`.
    ///
    /// The `&`-return makes the plain
    /// [`IrBuilder`](super::ir_builder::IrBuilder)'s self-consuming
    /// methods (terminators, repositioning) structurally unreachable
    /// through this handle -- the `SsaBuilder` never surrenders its
    /// positioned builder, which keeps its CFG bookkeeping (edges/fill
    /// state) complete. Reaching a terminator or a reposition requires
    /// going through this type's own terminator methods below, each of
    /// which records the bookkeeping the inner call alone would skip.
    ///
    /// What *did* change in cycle D is only the failure mode: an
    /// unpositioned builder used to be a different type (so this was an
    /// `E0599`), and now reports [`IrError::SsaUnpositioned`].
    pub fn ins(&self) -> IrResult<&super::ir_builder::IrBuilder<'ctx, 'ctx, B, F, Positioned, R>> {
        self.cursor.as_ref().ok_or(IrError::SsaUnpositioned)
    }

    /// The block the cursor names, as a copyable [`SsaBlock`] handle
    /// (usable as a branch target / phi predecessor elsewhere in this
    /// session's surface). [`IrError::SsaUnpositioned`] when the cursor
    /// is empty.
    pub fn current_block(&self) -> IrResult<SsaBlock<R, B>> {
        Ok(SsaBlock {
            id: self.ins()?.insert_block().id(),
            owner: self.state.id,
        })
    }

    /// [`ValueSlot`] of the block the cursor names -- the Braun engine's
    /// block key.
    #[inline]
    fn current_block_id(&self) -> IrResult<ValueSlot> {
        Ok(self.ins()?.insert_block().slot())
    }

    /// Braun `writeVariable`: pure bookkeeping, no IR emitted.
    ///
    /// D11: the engine's trivial-phi RAUW (`try_remove_trivial_phi`)
    /// assumes every value ever written for a variable shares that
    /// variable's pinned `ty`. This is the one seam where an external
    /// value enters `current_def`, so it is where that invariant is
    /// established -- for **every** marker, static ones included.
    ///
    /// A static `W` does not establish it by itself. `IntoIntValue<W>` is
    /// the *identity* on `IntValue<W>` (`int_width.rs`), so the lift adds
    /// no check of its own, and the crate-internal
    /// `IntValue::from_value_unchecked` mints an `IntValue<W>` without
    /// consulting the payload's real type. An in-crate caller can
    /// therefore hand a handle whose IR type contradicts `W` straight to
    /// this method. Keying the check on `W::static_bits().is_none()`
    /// would trust the very claim it exists to verify -- the caller's word
    /// that `W` matches the payload -- so the check is unconditional
    /// (`def_int_var_rejects_forged_static_width_handle` locks the static
    /// half; `tests/ssa_builder.rs`'s `dyn_int_var_wrong_width_def_rejected`
    /// the dyn one).
    ///
    /// No false rejections at a static `W`: [`Self::declare_int_var`]
    /// pins `var.ty` to `W::ir_type(..)`, and types are interned by width
    /// (`llvm_context.rs` memoizes `int_type(bits)`), so a correctly typed
    /// value always compares equal. For a dyn-declared `var`
    /// ([`Self::declare_int_var_dyn`], `W = IntDyn`) the marker proves
    /// only "some integer width" while `var.ty` names the actual one --
    /// `IntoIntValue<IntDyn>` happily lifts a DIFFERENT width than `var.ty`
    /// pins, which is the case this check was originally written for.
    pub fn def_int_var<W: IntWidth, V>(&mut self, var: IntVariable<W, B>, value: V) -> IrResult<()>
    where
        V: IntoIntValue<'ctx, W, B>,
    {
        self.check_owner_var(var.owner)?;
        let v = value.into_int_value(self.module_ref())?;
        super::r#type::Type::new(var.ty, self.module_ref()).require_match(v.into_erased().ty())?;
        let block = self.current_block_id()?;
        self.write_variable(var.index, block, v.slot());
        Ok(())
    }

    /// Braun `readVariable`; the result type reflects the declared
    /// variable (D4), sound because every writer of this variable's
    /// `current_def` entries was type-checked against `var.ty` at
    /// `def_int_var` time, at every marker -- see that method's doc
    /// comment for the full argument.
    pub fn use_int_var<W: IntWidth>(
        &mut self,
        var: IntVariable<W, B>,
    ) -> IrResult<IntValue<'ctx, W, B>> {
        self.check_owner_var(var.owner)?;
        let block = self.current_block_id()?;
        let id = self.read_variable_in(var.index, block)?;
        let value = Value::from_parts(id, self.module_ref(), var.ty);
        Ok(IntValue::from_value_unchecked(value))
    }

    /// Float twin of [`Self::def_int_var`]; see that method's doc comment
    /// for the full type-check rationale, which mirrors here exactly --
    /// including its reason for checking every marker rather than only the
    /// erased `FloatDyn` one: `FloatValue::from_value_unchecked` forges a
    /// static `K` just as freely as `IntValue`'s does a static `W`, and
    /// `IntoFloatValue<K>` is likewise the identity on `FloatValue<K>`
    /// (`def_float_var_rejects_forged_static_kind_handle` locks the static
    /// half).
    pub fn def_float_var<K: FloatKind, V>(
        &mut self,
        var: FloatVariable<K, B>,
        value: V,
    ) -> IrResult<()>
    where
        V: IntoFloatValue<'ctx, K, B>,
    {
        self.check_owner_var(var.owner)?;
        let v = value.into_float_value(self.module_ref())?;
        super::r#type::Type::new(var.ty, self.module_ref()).require_match(Typed::ty(v))?;
        let block = self.current_block_id()?;
        self.write_variable(var.index, block, v.slot());
        Ok(())
    }

    /// Float twin of [`Self::use_int_var`].
    pub fn use_float_var<K: FloatKind>(
        &mut self,
        var: FloatVariable<K, B>,
    ) -> IrResult<FloatValue<'ctx, K, B>> {
        self.check_owner_var(var.owner)?;
        let block = self.current_block_id()?;
        let id = self.read_variable_in(var.index, block)?;
        let value = Value::from_parts(id, self.module_ref(), var.ty);
        Ok(FloatValue::from_value_unchecked(value))
    }

    /// Pointer twin of [`Self::def_int_var`]. Pointer variables pin an
    /// address space via `var.ty` (`declare_pointer_var_in_addrspace`),
    /// but [`PointerValue`] does not statically pin its address space --
    /// `IntoPointerValue` happily lifts a pointer of ANY address space.
    /// This side therefore never had a static marker to key on, and is
    /// unconditional for the same reason the int and float sides now are:
    /// a `TypeSlot` equality compare is negligible next to the rest of the
    /// work, and skipping it would silently accept a wrong-address-space
    /// write. Honest rather than optimised.
    ///
    /// A drift here reports [`IrError::AddressSpaceMismatch`], not the
    /// `TypeMismatch { expected: Pointer, got: Pointer }` that
    /// [`TypeKindLabel`](crate::TypeKindLabel)'s single, address-space-less
    /// `Pointer` variant would otherwise force -- the pointer analogue of
    /// the int side's `OperandWidthMismatch`, and the reason this seam can
    /// now share `Type::require_match` with its twins instead of spelling
    /// its own compare (`pointer_var_wrong_addrspace_def_rejected`,
    /// `tests/ssa_builder.rs`, names both address spaces).
    pub fn def_pointer_var<V>(&mut self, var: PointerVariable<B>, value: V) -> IrResult<()>
    where
        V: IntoPointerValue<'ctx, B>,
    {
        self.check_owner_var(var.owner)?;
        let v = value.into_pointer_value(self.module_ref())?;
        super::r#type::Type::new(var.ty, self.module_ref()).require_match(Typed::ty(v))?;
        let block = self.current_block_id()?;
        self.write_variable(var.index, block, v.slot());
        Ok(())
    }

    /// Pointer twin of [`Self::use_int_var`].
    pub fn use_pointer_var(&mut self, var: PointerVariable<B>) -> IrResult<PointerValue<'ctx, B>> {
        self.check_owner_var(var.owner)?;
        let block = self.current_block_id()?;
        let id = self.read_variable_in(var.index, block)?;
        let value = Value::from_parts(id, self.module_ref(), var.ty);
        Ok(PointerValue::from_value_unchecked(value))
    }

    // ---- Terminators ----
    //
    // Each terminator mints a plain positioned builder for the cursor
    // (`ins`, which reports `SsaUnpositioned` if the cursor is empty),
    // delegates to that builder's OWN consuming terminator (which does
    // the actual IR emission and block-termination bookkeeping), records
    // the CFG edge(s) the Braun engine needs (`preds`, in the exact
    // order phi incoming operands should later be added in), marks the
    // source block filled, and clears the cursor -- so construction
    // continues at whichever block is switched to next, and a second
    // terminator on the same block is `SsaUnpositioned` (or, after an
    // explicit re-switch, `SsaBlockAlreadyFilled`).
    //
    // "ANY edge into a Braun-sealed block is an error" both enforces
    // Braun's own precondition (a sealed block's predecessor set is
    // final) and, since `create_block`'s first call auto-seals the
    // entry block, doubles as `Verifier::visitFunction`'s "entry has no
    // predecessors" check -- at construction time rather than at
    // `verify()` time.

    /// Produce `br label %dest`. Mirrors `IrBuilder::CreateBr`.
    pub fn br(&mut self, dest: SsaBlock<R, B>) -> IrResult<()> {
        self.check_owner_block(&dest)?;
        let dest_id = dest.id.slot();
        if self.state.sealed.contains(&dest_id) {
            return Err(IrError::SsaBranchToSealedBlock {
                block: block_name(self.module_ref(), dest_id),
            });
        }
        let src_id = self.current_block_id()?;
        let inner = self.cursor.take().ok_or(IrError::SsaUnpositioned)?;
        let (_terminated, _inst) = inner.br(dest.id)?;
        self.state.preds.entry(dest_id).or_default().push(src_id);
        self.state.filled.insert(src_id);
        Ok(())
    }

    /// Produce `br i1 <cond>, label %then, label %else`. Mirrors
    /// `IrBuilder::CreateCondBr`. Records the then-edge before the
    /// else-edge -- a phi at a block reachable from both arms sees its
    /// incoming operands added in the same order once each predecessor
    /// is later completed.
    pub fn cond_br<C>(
        &mut self,
        cond: C,
        then_dest: SsaBlock<R, B>,
        else_dest: SsaBlock<R, B>,
    ) -> IrResult<()>
    where
        C: IntoIntValue<'ctx, bool, B>,
    {
        self.check_owner_block(&then_dest)?;
        self.check_owner_block(&else_dest)?;
        let then_id = then_dest.id.slot();
        let else_id = else_dest.id.slot();
        if self.state.sealed.contains(&then_id) {
            return Err(IrError::SsaBranchToSealedBlock {
                block: block_name(self.module_ref(), then_id),
            });
        }
        if self.state.sealed.contains(&else_id) {
            return Err(IrError::SsaBranchToSealedBlock {
                block: block_name(self.module_ref(), else_id),
            });
        }
        let src_id = self.current_block_id()?;
        let inner = self.cursor.take().ok_or(IrError::SsaUnpositioned)?;
        let (_terminated, _inst) = inner.cond_br(cond, then_dest.id, else_dest.id)?;
        self.state.preds.entry(then_id).or_default().push(src_id);
        self.state.preds.entry(else_id).or_default().push(src_id);
        self.state.filled.insert(src_id);
        Ok(())
    }

    /// Produce `switch <cond>, label %default [ <case> label %dest ... ]`.
    /// Mirrors `IrBuilder::CreateSwitch` followed by the closed-form
    /// `SwitchInst::add_case` chain. `cases` is collected up front
    /// (closed form: every destination edge is observed by the time
    /// this method records any of them) and each `(value, target)` is
    /// added, then the switch is `finish()`ed.
    ///
    /// Edges are recorded per case OCCURRENCE, duplicates preserved:
    /// the default counts once, and each entry in `cases` counts once
    /// more, even if two entries target the SAME block -- matching
    /// `crate::cfg::FunctionCfg`'s switch successor list (default, then
    /// every case target in order, no deduplication), which is exactly
    /// what the verifier's `build_predecessors` counts phi incoming
    /// entries against ("entry-count must equal predecessor-count with
    /// multiplicity").
    ///
    /// Case constants are statically bound to the SAME width `W` as
    /// `cond` (`C: IntoConstantInt<'ctx, W, B>`) rather than accepting
    /// any [`IsValue`] -- a mismatched-width case is a *compile* error,
    /// not the runtime `TypeMismatch` `SwitchInst::add_case` would
    /// otherwise raise mid-loop, after the switch terminator (with its
    /// default target) has already been emitted. Every case is lifted
    /// and every destination's seal state is checked in a pre-pass,
    /// entirely BEFORE `self.inner` is taken -- so a lift failure (only
    /// reachable for an `IntDyn` `cond` whose case literal doesn't fit
    /// its runtime width) leaves `self` untouched and no IR emitted,
    /// instead of dropping a half-built builder over a partially-cased
    /// live switch.
    pub fn switch<W, V, C, Cases>(
        &mut self,
        cond: V,
        default_dest: SsaBlock<R, B>,
        cases: Cases,
    ) -> IrResult<()>
    where
        W: IntWidth,
        V: IntoIntValue<'ctx, W, B>,
        Cases: IntoIterator<Item = (C, SsaBlock<R, B>)>,
        C: IntoConstantInt<'ctx, W, B>,
        Result<ConstantIntValue<'ctx, W, B>, C::Error>: IntoIrResult<ConstantIntValue<'ctx, W, B>>,
    {
        self.check_owner_block(&default_dest)?;
        let cond = cond.into_int_value(self.module_ref())?;
        let cond_ty = cond.ty();
        let cases: Vec<(ConstantIntValue<'ctx, W, B>, SsaBlock<R, B>)> = cases
            .into_iter()
            .map(|(case_value, target)| {
                self.check_owner_block(&target)?;
                let lifted = case_value.into_constant_int(cond_ty).into_ir_result()?;
                Ok((lifted, target))
            })
            .collect::<IrResult<Vec<_>>>()?;
        let mut dest_ids = Vec::with_capacity(cases.len() + 1);
        let default_id = default_dest.id.slot();
        dest_ids.push(default_id);
        for (_, target) in &cases {
            dest_ids.push(target.id.slot());
        }
        for &dest_id in &dest_ids {
            if self.state.sealed.contains(&dest_id) {
                return Err(IrError::SsaBranchToSealedBlock {
                    block: block_name(self.module_ref(), dest_id),
                });
            }
        }
        let src_id = self.current_block_id()?;
        let inner = self.cursor.take().ok_or(IrError::SsaUnpositioned)?;
        let (_terminated, open) = inner.switch_dyn(cond, default_dest.id, "")?;
        let mut open = open;
        for (case_value, target) in cases {
            open = open.add_case(case_value, target.id).unwrap_or_else(|_| {
                unreachable!(
                    "SsaBuilder invariant: case_value was lifted via cond's own IntType in \
                     the pre-pass, so add_case's cond_ty == v.ty() check cannot fail here"
                )
            });
        }
        let _closed = open.finish();
        for &dest_id in &dest_ids {
            self.state.preds.entry(dest_id).or_default().push(src_id);
        }
        self.state.filled.insert(src_id);
        Ok(())
    }

    /// Produce `ret <value>`. Mirrors `IrBuilder::CreateRet`. Records no
    /// edges -- a `ret` has no successors.
    pub fn ret<V>(&mut self, value: V) -> IrResult<()>
    where
        V: IntoReturnValue<'ctx, R, B>,
    {
        let src_id = self.current_block_id()?;
        let inner = self.cursor.take().ok_or(IrError::SsaUnpositioned)?;
        let (_terminated, _inst) = inner.ret(value)?;
        self.state.filled.insert(src_id);
        Ok(())
    }

    /// Produce `unreachable`. Mirrors `IrBuilder::CreateUnreachable`.
    /// Records no edges. The inner `unreachable` is infallible, so
    /// the only failure here is an empty cursor.
    pub fn unreachable(&mut self) -> IrResult<()> {
        let src_id = self.current_block_id()?;
        let inner = self.cursor.take().ok_or(IrError::SsaUnpositioned)?;
        let (_terminated, _inst) = inner.unreachable();
        self.state.filled.insert(src_id);
        Ok(())
    }
}

impl<'s, 'ctx, B, F> SsaBuilder<'s, 'ctx, B, F, ()>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B> + Clone,
{
    /// Produce `ret void`. Mirrors `IrBuilder::CreateRetVoid`. Gated on
    /// the builder's return marker being statically `()`, matching the
    /// inner builder's own `ret_void` split (a [`Dyn`]-marker
    /// builder's `ret_void` would need a runtime parent-function check;
    /// no such builder shape is reachable here since `R = ()` is fixed
    /// by this impl block).
    pub fn ret_void(&mut self) -> IrResult<()> {
        let src_id = self.current_block_id()?;
        let inner = self.cursor.take().ok_or(IrError::SsaUnpositioned)?;
        let (_terminated, _inst) = inner.ret_void();
        self.state.filled.insert(src_id);
        Ok(())
    }
}

/// Shared field layout produced by [`SsaBuilder::declare_var_raw`]; each
/// public `IntVariable`/`FloatVariable`/`PointerVariable` constructor
/// below narrows this into its own phantom shape.
struct VarHandle<B: ModuleBrand> {
    index: u32,
    owner: SsaBuilderId,
    ty: TypeSlot,
    _b: Invariant<B>,
}

impl<B: ModuleBrand> From<VarHandle<B>> for PointerVariable<B> {
    #[inline]
    fn from(h: VarHandle<B>) -> Self {
        PointerVariable {
            index: h.index,
            owner: h.owner,
            ty: h.ty,
            _b: core::marker::PhantomData,
        }
    }
}

impl<W: IntWidth, B: ModuleBrand> From<VarHandle<B>> for IntVariable<W, B> {
    #[inline]
    fn from(h: VarHandle<B>) -> Self {
        IntVariable {
            index: h.index,
            owner: h.owner,
            ty: h.ty,
            _b: core::marker::PhantomData,
            _w: core::marker::PhantomData,
        }
    }
}

impl<K: FloatKind, B: ModuleBrand> From<VarHandle<B>> for FloatVariable<K, B> {
    #[inline]
    fn from(h: VarHandle<B>) -> Self {
        FloatVariable {
            index: h.index,
            owner: h.owner,
            ty: h.ty,
            _b: core::marker::PhantomData,
            _k: core::marker::PhantomData,
        }
    }
}

/// Emit a category-dispatched, name-only, operandless phi through
/// whichever positioned builder `emit_operandless_phi` has prepared for
/// the target insertion point, returning the raw [`ValueSlot`] of the new
/// phi instruction. `ty` is the declared variable's cached [`TypeSlot`];
/// `module` resolves it back to the category-appropriate typed handle
/// each dyn phi builder expects.
fn build_typed_phi<'m, 'ctx, B, F, R>(
    builder: &super::ir_builder::IrBuilder<'m, 'ctx, B, F, Positioned, R>,
    category: VarCategory,
    ty: TypeSlot,
    module: ModuleRef<'ctx, B>,
    name: &str,
) -> IrResult<ValueSlot>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B>,
    R: ReturnMarker,
{
    let id = match category {
        VarCategory::Int => {
            let int_ty = IntType::<super::int_width::IntDyn, B>::new(ty, module);
            let phi = builder.int_phi_dyn(int_ty, name)?;
            builder.view(phi).slot()
        }
        VarCategory::Float => {
            let float_ty = FloatType::<super::float_kind::FloatDyn, B>::new(ty, module);
            let phi = builder.fp_phi_dyn(float_ty, name)?;
            builder.view(phi).slot()
        }
        VarCategory::Pointer => {
            let ptr_ty = PointerType::<B>::new(ty, module);
            let phi = builder.pointer_phi_in_addrspace(ptr_ty, name)?;
            builder.view(phi).slot()
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
impl<'s, 'ctx, B, F, R> SsaBuilder<'s, 'ctx, B, F, R>
where
    B: ModuleBrand + 'ctx,
    F: IrBuilderFolder<'ctx, B> + Clone,
    R: ReturnMarker,
{
    /// Braun `writeVariable`.
    fn write_variable(&mut self, var: u32, block: ValueSlot, value: ValueSlot) {
        self.state.current_def.insert((block, var), value);
    }

    /// Braun `readVariable` + `readVariableRecursive`, restated
    /// iteratively for the single-predecessor chase.
    ///
    /// D11: the paper ends EVERY `readVariableRecursive` branch with
    /// `writeVariable(variable, block, val)` -- including the
    /// single-predecessor case -- memoizing the resolved value at each
    /// block the chase passed through. `chased` accumulates those
    /// intermediate blocks (never the origin block itself, which either
    /// already had a `current_def` hit -- nothing to write -- or is about
    /// to receive its OWN fresh entry via `write_variable`/`add_phi_operands`
    /// below); whichever branch below resolves the read writes the same
    /// resolved id back to every block in `chased` before returning, so a
    /// second read from any point on the chain is O(1) instead of
    /// re-chasing the whole straight-line run.
    fn read_variable_in(&mut self, var: u32, mut block: ValueSlot) -> IrResult<ValueSlot> {
        let mut chased: Vec<ValueSlot> = Vec::new();
        loop {
            if let Some(v) = self.state.current_def.get(&(block, var)) {
                let resolved = self.resolve(*v);
                self.memoize_chase(var, &chased, resolved);
                return Ok(resolved);
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
                self.memoize_chase(var, &chased, phi);
                return Ok(phi);
            }
            let preds = self.state.preds.get(&block).cloned().unwrap_or_default();
            match preds.len() {
                0 => {
                    let resolved = self.undefined_read(var, block)?;
                    self.memoize_chase(var, &chased, resolved);
                    return Ok(resolved);
                }
                1 => {
                    // A revisit means the chase entered a CLOSED cycle of
                    // sealed single-pred blocks with no def anywhere on it
                    // -- only constructible in an unreachable region (any
                    // cycle reachable from entry has a >=2-pred block that
                    // breaks the chase via its operandless phi). Braun's
                    // recursion diverges on this input too; route it to the
                    // undefined-read handling instead of chasing forever.
                    if chased.contains(&block) {
                        let resolved = self.undefined_read(var, block)?;
                        self.memoize_chase(var, &chased, resolved);
                        return Ok(resolved);
                    }
                    // Single-pred chase: no phi needed at `block` itself,
                    // but `block` is now part of the chain whose resolved
                    // value will be memoized once the chase terminates.
                    chased.push(block);
                    block = preds[0];
                }
                _ => {
                    let phi = self.emit_operandless_phi(var, block)?;
                    self.write_variable(var, block, phi); // breaks cycles
                    let resolved = self.add_phi_operands(var, phi, block)?;
                    self.memoize_chase(var, &chased, resolved);
                    return Ok(resolved);
                }
            }
        }
    }

    /// Write `resolved` back into `current_def` for every block the
    /// single-predecessor chase passed through, per `read_variable_in`'s
    /// doc comment. `chased` never contains the block that actually
    /// produced `resolved` (that block already has its own correct
    /// `current_def`/`incomplete_phis` entry from the branch above), so
    /// every write here is a genuinely new memoization rather than a
    /// redundant overwrite.
    fn memoize_chase(&mut self, var: u32, chased: &[ValueSlot], resolved: ValueSlot) {
        for &block in chased {
            self.write_variable(var, block, resolved);
        }
    }

    /// Braun `addPhiOperands` + `tryRemoveTrivialPhi`.
    fn add_phi_operands(
        &mut self,
        var: u32,
        phi: ValueSlot,
        block: ValueSlot,
    ) -> IrResult<ValueSlot> {
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
    fn try_remove_trivial_phi(&mut self, phi: ValueSlot) -> IrResult<ValueSlot> {
        let mut same: Option<ValueSlot> = None;
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
        let users: Vec<ValueSlot> = self.phi_user_ids(phi);
        self.state.phi_var.remove(&phi);
        if !self.state.created_phis.remove(&phi) {
            unreachable!(
                "SsaBuilder invariant: every ValueSlot reachable through try_remove_trivial_phi \
                 was produced by Self::emit_operandless_phi, which always records its slot in \
                 created_phis before returning"
            )
        }
        let module = self.module_ref();
        // Re-mint the linear lifecycle handle from the slot: cycle D
        // stopped `SsaState` storing one so it could shed its `'ctx`.
        // The slot came out of `created_phis`, so it names a phi this
        // layer created and has not yet erased.
        let handle = Instruction::<Attached, B>::from_parts(phi, module);
        let same_ty = module.value_data(same).ty;
        let replacement = Value::from_parts(same, module, same_ty);
        // `replace_all_uses_with`'s only failure mode is a type mismatch
        // between the phi's cached result type and `replacement`'s type
        // (instruction.rs). `same` is one of this very phi's own incoming
        // operands (the loop above only ever assigns `same` from
        // `self.phi_incoming_values(phi)`). The dyn path this engine uses
        // (`phi_add_incoming_raw` -> `IrBuilder::phi_add_incoming_from_value`,
        // ir_builder.rs) now performs the same result-type check the typed
        // `PhiInst::add_incoming` does, at the call site rather than only at
        // `Module::verify` (belt-and-braces: Braun reads are same-typed by
        // construction, so that check never fires on this engine's writes).
        // The narrower guarantee this `unreachable!` actually relies on is
        // currently-true-by-construction and not (yet) checked at the
        // phi-mutation call site:
        // every operand this engine has EVER pushed onto a layer-created
        // phi's incoming list is either (a) another layer-created phi's
        // own id (`emit_operandless_phi` always builds it from the same
        // declared variable's `VarData.ty`), or (b) `undefined_read` /
        // `undefined_phi_replacement`'s poison value (built from that same
        // `VarData.ty`), or (c) a value passed to `write_variable` -- and
        // this task ships no public write path, so every current
        // `write_variable` call site (inside this same file) only ever
        // writes a value already known same-typed. A future public def/use API,
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
            if self.state.created_phis.contains(&user) {
                self.try_remove_trivial_phi(user)?;
            }
        }
        Ok(self.resolve(same))
    }

    /// Path-compressed forwarding lookup: chase the `resolved` chain built
    /// by [`Self::try_remove_trivial_phi`] and flatten it so future
    /// lookups are O(1).
    fn resolve(&self, mut v: ValueSlot) -> ValueSlot {
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

    /// Emit an operandless phi at the phi head of `block`. The phi
    /// builders insert after the block's leading phi run regardless of the
    /// throwaway builder's cursor (see `append_phi_instruction`), so this
    /// method only has to position a builder *inside* `block` — the exact
    /// cursor within it does not affect where the phi lands. That
    /// collapses to two cases, keyed purely on emptiness — cycle D no
    /// longer has to ask which of the live builder / an `open_blocks`
    /// map owns the block's linear handle, because it keeps no such
    /// handle at all:
    ///
    /// - `block` has >= 1 instruction already (whether it is open,
    ///   current, or filled/terminated): a fresh throwaway builder
    ///   positioned via `position_before(&first_instruction)` derives
    ///   its own insertion block from the anchor's parent, so no linear
    ///   `BasicBlock` handle is needed at all here.
    /// - `block` is empty: `position_before` has no anchor to derive
    ///   from, so head-insertion needs an actual end-of-block position,
    ///   which requires an `Unterminated` `BasicBlock` handle. It is
    ///   re-minted from the slot. That is sound *because* this arm is
    ///   reached only when the block has no instructions: an empty block
    ///   provably has no terminator, so the `Unterminated` claim the
    ///   handle makes is checked, not assumed.
    fn emit_operandless_phi(&mut self, var: u32, block: ValueSlot) -> IrResult<ValueSlot> {
        let idx = usize::try_from(var).unwrap_or_else(|_| {
            unreachable!("SsaBuilder invariant: var indices are u32::try_from(vars.len())")
        });
        let var_ty = self.state.vars[idx].ty;
        let var_category = self.state.vars[idx].category;
        let var_name = self.state.vars[idx].name.clone();
        let module = self.module_ref();
        let label_ty = module.module().label_type::<B>().as_type().id();

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
            let builder: super::ir_builder::IrBuilder<'_, 'ctx, B, F, Positioned, Dyn> =
                super::ir_builder::IrBuilder::with_folder(self.module, self.folder.clone())
                    .position_before(&anchor);
            build_typed_phi(&builder, var_category, var_ty, module, &var_name)?
        } else {
            // Empty: end-of-block IS head-of-block, and an empty block
            // has no terminator, so re-minting the `Unterminated` handle
            // from the slot states only what the emptiness check just
            // proved.
            let open = BasicBlock::<Dyn, Unterminated, B>::from_parts(block, module, label_ty);
            let positioned: super::ir_builder::IrBuilder<'_, 'ctx, B, F, Positioned, Dyn> =
                super::ir_builder::IrBuilder::with_folder(self.module, self.folder.clone())
                    .position_at_end(open);
            build_typed_phi(&positioned, var_category, var_ty, module, &var_name)?
        };
        self.state.created_phis.insert(inst);
        self.state.phi_var.insert(inst, var);
        Ok(inst)
    }

    /// Add `(operand, pred)` to the layer-created phi named by `phi`.
    /// Thin wrapper over the same dyn phi-mutation idiom
    /// `IrBuilder::phi_add_incoming_from_value` uses, since the engine
    /// only ever holds category-erased `ValueSlot`s. Pinned to `Dyn`: the
    /// return-marker parameter is irrelevant to a payload-only mutation
    /// that never emits a terminator.
    fn phi_add_incoming_raw(
        &self,
        phi: ValueSlot,
        operand: ValueSlot,
        pred: ValueSlot,
    ) -> IrResult<()> {
        let module = self.module_ref();
        let phi_value = Value::from_parts(phi, module, module.value_data(phi).ty);
        let operand_value = Value::from_parts(operand, module, module.value_data(operand).ty);
        let label_ty = module.module().label_type::<B>().as_type().id();
        let pred_block = BasicBlock::<Dyn, Unterminated, B>::from_parts(pred, module, label_ty);
        let ib: super::ir_builder::IrBuilder<'_, 'ctx, B, F, super::ir_builder::Unpositioned, Dyn> =
            super::ir_builder::IrBuilder::with_folder(self.module, self.folder.clone());
        ib.phi_add_incoming_from_value(phi_value, operand_value, pred_block)
    }

    /// Read the current incoming-value list of a layer-created phi,
    /// resolved through the same value-arena path `PhiInst::payload`
    /// uses (category-agnostic: works for the int/float/pointer phi
    /// handles alike, since they all share `InstructionKindData::Phi`).
    fn phi_incoming_values(&self, phi: ValueSlot) -> Vec<ValueSlot> {
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
    fn phi_user_ids(&self, phi: ValueSlot) -> Vec<ValueSlot> {
        let module = self.module_ref();
        let value = Value::from_parts(phi, module, module.value_data(phi).ty);
        value.users().map(|u| u.slot()).collect()
    }

    /// A strict variable's read reached function entry with no write on
    /// the path: `Err(SsaUseOfUndefinedVariable)`. A poison-on-undef
    /// variable instead materialises `poison <ty>` for that read.
    fn undefined_read(&mut self, var: u32, block: ValueSlot) -> IrResult<ValueSlot> {
        let idx = usize::try_from(var).unwrap_or_else(|_| {
            unreachable!("SsaBuilder invariant: var indices are u32::try_from(vars.len())")
        });
        let data = &self.state.vars[idx];
        if data.poison_on_undef {
            let module = self.module_ref();
            let ty = super::r#type::Type::new(data.ty, module);
            let poison = ty.get_poison();
            return Ok(poison.slot());
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
    fn undefined_phi_replacement(&mut self, phi: ValueSlot) -> IrResult<ValueSlot> {
        let module = self.module_ref();
        let ty = module.value_data(phi).ty;
        if !self.state.created_phis.contains(&phi) {
            unreachable!(
                "SsaBuilder invariant: try_remove_trivial_phi only calls this helper on a \
                 phi still present in created_phis"
            )
        }
        let block_id = Instruction::<Attached, B>::from_parts(phi, module)
            .parent()
            .slot();
        // Recover which declared variable this phi belongs to via
        // `phi_var`, populated alongside `created_phis` in
        // `emit_operandless_phi` (the one place that KNOWS which
        // variable it is building the phi for). D10: a type-only search
        // over `vars` is ambiguous whenever two variables share a type
        // (e.g. a poison i32 and a strict i32) -- it would silently
        // return the FIRST matching declaration regardless of which
        // variable's phi is actually being resolved, letting a strict
        // variable's dead-cycle read resolve to poison (or a poison
        // variable's error misname the wrong variable).
        let var_idx = *self.state.phi_var.get(&phi).unwrap_or_else(|| {
            unreachable!(
                "SsaBuilder invariant: every layer-created phi's index was recorded in phi_var \
                 by Self::emit_operandless_phi"
            )
        });
        let var = &self.state.vars[usize::try_from(var_idx).unwrap_or_else(|_| {
            unreachable!("SsaBuilder invariant: var indices are u32::try_from(vars.len())")
        })];
        if var.poison_on_undef {
            let poison_ty = super::r#type::Type::new(ty, module);
            let poison = poison_ty.get_poison();
            // Braun's `same == None` arm still runs `phi.replaceBy(same)`:
            // reroute every user to the poison constant BEFORE erasing, or
            // surviving instructions keep an operand naming an erased
            // value. Same snapshot-users / RAUW / erase / re-check-users
            // shape as the trivial path in `try_remove_trivial_phi`.
            let users: Vec<ValueSlot> = self.phi_user_ids(phi);
            self.state.phi_var.remove(&phi);
            if !self.state.created_phis.remove(&phi) {
                unreachable!(
                    "SsaBuilder invariant: undefined_phi_replacement read this phi's block out \
                     of created_phis a few lines above"
                )
            }
            Instruction::<Attached, B>::from_parts(phi, module)
                .replace_all_uses_with(self.module, poison.into_erased())
                .unwrap_or_else(|_| {
                    unreachable!(
                        "SsaBuilder invariant: the poison constant is built from the phi's own \
                         result type, so the RAUW type check cannot fail"
                    )
                });
            Instruction::<Attached, B>::from_parts(phi, module).erase_from_parent(self.module);
            let resolved = poison.slot();
            self.state.resolved.borrow_mut().insert(phi, resolved);
            for user in users {
                if self.state.created_phis.contains(&user) {
                    self.try_remove_trivial_phi(user)?;
                }
            }
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
    use crate::Linkage;

    /// llvmkit-specific: no upstream C++ equivalent (LLVM's `IrBuilder`
    /// has no on-the-fly SSA layer -- the closest functional relative is
    /// `SSAUpdater::Initialize`, which likewise treats the first block it
    /// sees as needing no predecessor completion). Locks that
    /// `create_block`'s FIRST call auto-seals the entry block, matching
    /// `Verifier::visitFunction`'s invariant that the entry block has no
    /// predecessors.
    #[test]
    fn first_created_block_is_auto_sealed() -> Result<(), IrError> {
        let m = crate::module_new!("ssa-entry-seal")?;
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let mut st_b = SsaState::for_function(&m, m.view(f))?;
        let mut b = SsaBuilder::for_function(&m, m.view(f), &mut st_b)?;
        let entry = b.create_block("entry");
        let entry_id = entry.id.slot();
        assert!(b.state.sealed.contains(&entry_id));

        // A second block is NOT auto-sealed.
        let second = b.create_block("second");
        let second_id = second.id.slot();
        assert!(!b.state.sealed.contains(&second_id));
        Ok(())
    }

    /// llvmkit-specific: locks `seal_block`'s double-seal rejection
    /// (Braun's algorithm assumes each block is sealed exactly once,
    /// after which its predecessor set is considered final).
    #[test]
    fn seal_block_twice_errors() -> Result<(), IrError> {
        let m = crate::module_new!("ssa-double-seal")?;
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let mut st_b = SsaState::for_function(&m, m.view(f))?;
        let mut b = SsaBuilder::for_function(&m, m.view(f), &mut st_b)?;
        let _entry = b.create_block("entry");
        let second = b.create_block("second"); // not entry -- unsealed
        b.seal_block(second)?;
        match b.seal_block(second) {
            Err(IrError::SsaBlockAlreadySealed { .. }) => {}
            other => panic!("expected SsaBlockAlreadySealed, got {other:?}"),
        }
        Ok(())
    }

    /// llvmkit-specific: locks `SsaFunctionHasBlocks` -- the layer must
    /// observe every CFG edge from birth, so grafting onto a function
    /// that already has a body is rejected rather than silently missing
    /// the pre-existing blocks' edges. Cycle D moved the check to
    /// [`SsaState::for_function`], the one place a session is *opened*:
    /// `SsaBuilder::for_function` re-mints a working handle over an
    /// in-progress session and must obviously not re-trip it.
    #[test]
    fn for_function_rejects_function_with_existing_blocks() -> Result<(), IrError> {
        let m = crate::module_new!("ssa-nonempty-fn")?;
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let _entry = m.view(f).append_basic_block(&m, "entry");
        match SsaState::for_function(&m, m.view(f)) {
            Err(IrError::SsaFunctionHasBlocks) => {}
            Ok(_) => panic!("expected SsaFunctionHasBlocks, got Ok"),
            Err(other) => panic!("expected SsaFunctionHasBlocks, got {other:?}"),
        }
        Ok(())
    }

    /// llvmkit-specific: locks `SsaForeignBlock` -- a block handle from a
    /// different `SsaBuilder` is a typed runtime error at `seal_block`.
    #[test]
    fn seal_block_rejects_foreign_block() -> Result<(), IrError> {
        let m = crate::module_new!("ssa-foreign-block")?;
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f1 = m.add_function_dyn("f1", fn_ty, Linkage::External)?;
        let f2 = m.add_function_dyn("f2", fn_ty, Linkage::External)?;
        let mut st_b1 = SsaState::for_function(&m, m.view(f1))?;
        let mut b1 = SsaBuilder::for_function(&m, m.view(f1), &mut st_b1)?;
        let _entry1 = b1.create_block("entry");
        let other1 = b1.create_block("other");

        let mut st_b2 = SsaState::for_function(&m, m.view(f2))?;
        let mut b2 = SsaBuilder::for_function(&m, m.view(f2), &mut st_b2)?;
        let _entry2 = b2.create_block("entry");

        match b2.seal_block(other1) {
            Err(IrError::SsaForeignBlock) => {}
            other => panic!("expected SsaForeignBlock, got {other:?}"),
        }
        Ok(())
    }

    /// llvmkit-specific: locks the declared-variable handle shape (the
    /// `owner` accessor) across all three categories. The old `module()`
    /// accessor is gone with cycle D's lifetime-free variable handles:
    /// the owning module is pinned by the brand type parameter, and the
    /// handle stores no `ModuleRef` to report.
    #[test]
    fn declare_var_family_reports_owner() -> Result<(), IrError> {
        let m = crate::module_new!("ssa-declare")?;
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let mut st_b = SsaState::for_function(&m, m.view(f))?;
        let mut b = SsaBuilder::for_function(&m, m.view(f), &mut st_b)?;
        let int_var = b.declare_int_var::<i32, _>("x");
        let float_var = b.declare_float_var::<f64, _>("y");
        let ptr_var = b.declare_pointer_var("z");
        assert_eq!(int_var.owner(), b.id());
        assert_eq!(float_var.owner(), b.id());
        assert_eq!(ptr_var.owner(), b.id());
        assert_eq!(b.state.vars.len(), 3);
        Ok(())
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
        let m = crate::module_new!("ssa-straight-line")?;
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let mut st_b = SsaState::for_function(&m, m.view(f))?;
        let mut b = SsaBuilder::for_function(&m, m.view(f), &mut st_b)?;
        let entry = b.create_block("entry");
        let entry_id = entry.id.slot();

        let var: IntVariable<i32, _> = b.declare_int_var("x");
        let one = m.i32_type().const_int(1_i32).slot();
        b.write_variable(var.index, entry_id, one);
        let read = b.read_variable_in(var.index, entry_id)?;
        assert_eq!(read, one);
        assert!(b.state.created_phis.is_empty());
        Ok(())
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
        let m = crate::module_new!("ssa-incomplete-phi")?;
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let mut st_b = SsaState::for_function(&m, m.view(f))?;
        let mut b = SsaBuilder::for_function(&m, m.view(f), &mut st_b)?;
        let _entry = b.create_block("entry");
        let entry_id = _entry.id.slot();
        let loop_bb = b.create_block("loop");
        let loop_id = loop_bb.id.slot();

        // Record edges: entry -> loop, loop -> loop (self back-edge).
        b.state.preds.entry(loop_id).or_default().push(entry_id);
        b.state.preds.entry(loop_id).or_default().push(loop_id);

        let var: IntVariable<i32, _> = b.declare_int_var("i");
        let zero = m.i32_type().const_int(0_i32).slot();
        b.write_variable(var.index, entry_id, zero);

        // Read inside the not-yet-sealed loop block: creates an
        // incomplete (operandless) phi and records it for later
        // completion.
        let read_before_seal = b.read_variable_in(var.index, loop_id)?;
        assert_eq!(b.state.incomplete_phis.get(&loop_id).map(Vec::len), Some(1));
        assert!(b.state.created_phis.contains(&read_before_seal));

        // Record the loop body's own write (e.g. `i + 1`, modeled
        // here as reusing a fresh constant is fine -- the engine
        // does not care what the value IS, only that a def exists).
        let one = m.i32_type().const_int(1_i32).slot();
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
        let m = crate::module_new!("ssa-trivial-join")?;
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let mut st_b = SsaState::for_function(&m, m.view(f))?;
        let mut b = SsaBuilder::for_function(&m, m.view(f), &mut st_b)?;
        let _entry = b.create_block("entry");
        let entry_id = _entry.id.slot();
        let left = b.create_block("left");
        let left_id = left.id.slot();
        let right = b.create_block("right");
        let right_id = right.id.slot();
        let join = b.create_block("join");
        let join_id = join.id.slot();

        b.state.preds.entry(left_id).or_default().push(entry_id);
        b.state.preds.entry(right_id).or_default().push(entry_id);
        b.state.preds.entry(join_id).or_default().push(left_id);
        b.state.preds.entry(join_id).or_default().push(right_id);
        b.seal_block(left)?;
        b.seal_block(right)?;

        let var: IntVariable<i32, _> = b.declare_int_var("x");
        let same_value = m.i32_type().const_int(7_i32).slot();
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
        let m = crate::module_new!("ssa-undefined-strict")?;
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let mut st_b = SsaState::for_function(&m, m.view(f))?;
        let mut b = SsaBuilder::for_function(&m, m.view(f), &mut st_b)?;
        let entry = b.create_block("entry");
        let entry_id = entry.id.slot();

        let var: IntVariable<i32, _> = b.declare_int_var("x");
        match b.read_variable_in(var.index, entry_id) {
            Err(IrError::SsaUseOfUndefinedVariable { .. }) => {}
            other => panic!("expected SsaUseOfUndefinedVariable, got {other:?}"),
        }
        Ok(())
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
        let m = crate::module_new!("ssa-undefined-poison")?;
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let mut st_b = SsaState::for_function(&m, m.view(f))?;
        let mut b = SsaBuilder::for_function(&m, m.view(f), &mut st_b)?;
        let entry = b.create_block("entry");
        let entry_id = entry.id.slot();

        let var: IntVariable<i32, _> = b.declare_int_var_poison("x");
        let read = b.read_variable_in(var.index, entry_id)?;
        let i32_ty = m.i32_type();
        let poison_id = i32_ty.as_type().get_poison().slot();
        assert_eq!(read, poison_id);
        Ok(())
    }

    /// Review follow-up (D11): Braun et al. 2013 SS2's `readVariableRecursive`
    /// ends EVERY branch -- including the single-predecessor chase -- with
    /// `writeVariable(variable, block, val)`, memoizing the resolved value
    /// at each block it passed through. Without that write-back, a repeated
    /// read at the END of a straight-line chain re-chases the WHOLE chain
    /// from scratch every time (O(chain length) per read, quadratic over a
    /// long straight-line function body). This locks the postcondition
    /// directly on `current_def`: a single `read_variable_in` at the far
    /// end of a 4-block sealed straight-line chain (entry def x -> b1 -> b2
    /// -> b3) must leave b1 and b2 (the blocks merely PASSED THROUGH, not
    /// just b3 itself) with their own `current_def` entry pointing at the
    /// resolved definition, so a second read from any point on the chain is
    /// O(1) instead of re-chasing. No upstream C++ analogue (`SSAUpdater`
    /// does not memoize the same way); this is a direct-from-the-paper
    /// llvmkit-specific white-box check.
    #[test]
    fn read_variable_in_memoizes_single_pred_chase() -> Result<(), IrError> {
        let m = crate::module_new!("ssa-chase-memoization")?;
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let mut st_b = SsaState::for_function(&m, m.view(f))?;
        let mut b = SsaBuilder::for_function(&m, m.view(f), &mut st_b)?;
        let entry = b.create_block("entry");
        let entry_id = entry.id.slot();
        let b1 = b.create_block("b1");
        let b1_id = b1.id.slot();
        let b2 = b.create_block("b2");
        let b2_id = b2.id.slot();
        let b3 = b.create_block("b3");
        let b3_id = b3.id.slot();

        // Straight-line chain: entry -> b1 -> b2 -> b3, each with a
        // single predecessor, all sealed as soon as their one edge is
        // known (Braun requires the full predecessor set before seal).
        b.state.preds.entry(b1_id).or_default().push(entry_id);
        b.seal_block(b1)?;
        b.state.preds.entry(b2_id).or_default().push(b1_id);
        b.seal_block(b2)?;
        b.state.preds.entry(b3_id).or_default().push(b2_id);
        b.seal_block(b3)?;

        let var: IntVariable<i32, _> = b.declare_int_var("x");
        let one = m.i32_type().const_int(1_i32).slot();
        b.write_variable(var.index, entry_id, one);

        // Before the read: only entry has a current_def entry.
        assert!(b.state.current_def.contains_key(&(entry_id, var.index)));
        assert!(!b.state.current_def.contains_key(&(b1_id, var.index)));
        assert!(!b.state.current_def.contains_key(&(b2_id, var.index)));

        let read = b.read_variable_in(var.index, b3_id)?;
        assert_eq!(read, one);

        // After the read: the intermediate blocks the chase passed
        // through (b1, b2) must now be memoized too, per the paper's
        // writeVariable postcondition at the end of every
        // readVariableRecursive branch.
        assert_eq!(
            b.state.current_def.get(&(b1_id, var.index)),
            Some(&one),
            "b1 should be memoized after the chase resolves through it"
        );
        assert_eq!(
            b.state.current_def.get(&(b2_id, var.index)),
            Some(&one),
            "b2 should be memoized after the chase resolves through it"
        );
        Ok(())
    }

    /// llvmkit-specific: no upstream C++ equivalent. LLVM's `SSAUpdater`
    /// carries no per-variable static width to contradict -- `AddAvailableValue`
    /// takes a bare `Value *` and the type agreement it needs is asserted, not
    /// typed (`SSAUpdater::AddAvailableValue`, `SSAUpdater.cpp`). The invariant
    /// this locks is the Rust layer's own: D11's requirement that every value
    /// written for a variable share that variable's pinned `ty`, which the
    /// trivial-phi RAUW (`try_remove_trivial_phi`) depends on.
    ///
    /// Locks [`SsaBuilder::def_int_var`] at a *static* `W` -- the half its old
    /// `W::static_bits().is_none() &&` short-circuit let through, and the same
    /// bug class the `IrBuilder` acceptors shed in `bf57e17`/`b5cb1e5`
    /// (`hostile_native_typed_override_wrong_width_rejected_at_static_width`,
    /// `ir_builder.rs`, is the fold-result analogue). The old doc argued the
    /// static case was safe because `IntValue<W>` proves the width -- circular,
    /// since `from_value_unchecked` exists precisely to mint that claim without
    /// consulting the payload, and `IntoIntValue<W>` is the identity on
    /// `IntValue<W>`, so nothing between the forge and `current_def` re-checks.
    ///
    /// Here `x` is declared `IntVariable<i32>` (so `var.ty` is pinned to `i32`
    /// by `declare_int_var`), but the value handed to `def_int_var` is an
    /// `IntValue<'_, i32, _>` whose real IR type is `i64`. Unlike the honest
    /// halves of the fixtures above, the lie is minted right at the call site
    /// on purpose: an in-crate caller forging the handle *is* the threat this
    /// seam exists to catch. Without the check the `i64` would land in
    /// `current_def` under an `i32`-pinned variable and a later `use_int_var`
    /// would hand back an `IntValue<'_, i32>` that is really an `i64`.
    #[test]
    fn def_int_var_rejects_forged_static_width_handle() -> Result<(), IrError> {
        let m = crate::module_new!("ssa-forged-static-width")?;
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let mut st_b = SsaState::for_function(&m, m.view(f))?;
        let mut b = SsaBuilder::for_function(&m, m.view(f), &mut st_b)?;
        let entry = b.create_block("entry");
        let x = b.declare_int_var::<i32, _>("x");

        b.switch_to_block(entry)?;
        let forged: IntValue<'_, i32, _> =
            IntValue::from_value_unchecked(m.i64_type().const_zero().into_erased());

        let err = b
            .def_int_var(x, forged)
            .expect_err("a forged static-width handle must be rejected");

        // Both sides are integers, so the widths are reported rather than a
        // `TypeMismatch { expected: Integer, got: Integer }` that could not
        // say which width was wrong (`Type::require_match`).
        assert_eq!(err, IrError::OperandWidthMismatch { lhs: 32, rhs: 64 });
        Ok(())
    }

    /// Float twin of [`def_int_var_rejects_forged_static_width_handle`],
    /// locking [`SsaBuilder::def_float_var`] at a *static* `K` -- the half its
    /// old `K::ieee_label().is_none() &&` short-circuit let through.
    /// `FloatValue::from_value_unchecked` forges a static `K` just as freely as
    /// `IntValue`'s does a static `W`, and `IntoFloatValue<K>` is likewise the
    /// identity on `FloatValue<K>`. llvmkit-specific for the same reason as its
    /// int twin.
    ///
    /// Stays [`IrError::TypeMismatch`]: `TypeKindLabel` has a distinct variant
    /// per float kind, so the labels already name both sides precisely -- the
    /// width-less `Integer` variant that forces the int side to report
    /// `OperandWidthMismatch` has no float counterpart
    /// (`Type::require_match`).
    #[test]
    fn def_float_var_rejects_forged_static_kind_handle() -> Result<(), IrError> {
        let m = crate::module_new!("ssa-forged-static-kind")?;
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let mut st_b = SsaState::for_function(&m, m.view(f))?;
        let mut b = SsaBuilder::for_function(&m, m.view(f), &mut st_b)?;
        let entry = b.create_block("entry");
        let x = b.declare_float_var::<f32, _>("x");

        b.switch_to_block(entry)?;
        let forged: FloatValue<'_, f32, _> =
            FloatValue::from_value_unchecked(m.f64_type().const_from_bits(0).into_erased());

        let err = b
            .def_float_var(x, forged)
            .expect_err("a forged static-kind handle must be rejected");

        assert_eq!(
            err,
            IrError::TypeMismatch {
                expected: crate::TypeKindLabel::Float,
                got: crate::TypeKindLabel::Double,
            }
        );
        Ok(())
    }

    /// Pointer twin of [`def_int_var_rejects_forged_static_width_handle`],
    /// completing the `def_*_var` trio. `PointerValue` pins no address space
    /// statically, so its forge lies about the *kind*: an honest non-pointer
    /// cannot reach [`SsaBuilder::def_pointer_var`] (there is no
    /// `IntoPointerValue` impl for `IntValue`), and
    /// `PointerValue::from_value_unchecked` is the only door in --
    /// crate-internal, and named as an `ssa_builder.rs` caller by that
    /// method's own doc, which cites `def_pointer_var`'s check as what makes
    /// the arena read (`use_pointer_var`) safe. This is that check.
    ///
    /// Stays [`IrError::TypeMismatch`] rather than the
    /// [`IrError::AddressSpaceMismatch`] its honest wrong-address-space
    /// sibling reports (`pointer_var_wrong_addrspace_def_rejected`,
    /// `tests/ssa_builder.rs`): the address-space arm fires only when BOTH
    /// sides are pointers, since an `i32` has no address space to name
    /// (`Type::require_match`). llvmkit-specific for the same reason as its
    /// int twin: `SSAUpdater` carries no per-variable type to contradict.
    #[test]
    fn def_pointer_var_rejects_forged_non_pointer_handle() -> Result<(), IrError> {
        let m = crate::module_new!("ssa-forged-pointer")?;
        let fn_ty = m.fn_type_no_params(m.void_type(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let mut st_b = SsaState::for_function(&m, m.view(f))?;
        let mut b = SsaBuilder::for_function(&m, m.view(f), &mut st_b)?;
        let entry = b.create_block("entry");
        let p = b.declare_pointer_var("p");

        b.switch_to_block(entry)?;
        let forged: PointerValue<'_, _> =
            PointerValue::from_value_unchecked(m.i32_type().const_zero().into_erased());

        let err = b
            .def_pointer_var(p, forged)
            .expect_err("a forged non-pointer handle must be rejected");

        assert_eq!(
            err,
            IrError::TypeMismatch {
                expected: crate::TypeKindLabel::Pointer,
                got: crate::TypeKindLabel::Integer,
            }
        );
        Ok(())
    }
}
