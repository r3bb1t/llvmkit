//! Pass-author context types.
//!
//! The pass drivers hand these narrow contexts to a pass's `run` instead of raw
//! module storage. A read-only ([`Inspect`]) context exposes verified views and
//! infallible analysis queries but no `mutate()`; a mutating rung's context
//! yields its mutator ([`FnPatch`]/[`FnReshape`]/[`ModRewrite`]) only through
//! the *consuming* [`FnCx::mutate`]/[`ModCx::mutate`], so once a pass has begun
//! mutating, the all-preserved report is no longer spellable (the context was
//! moved). The mutator carries an unverified-module capability
//! (`module_mut() -> &Module<Unverified>`) and its `done()` reports exactly the
//! rung's preservation floor.
//!
//! [`Inspect`]: crate::Inspect
//!
//! # Example: a mutating module pass
//!
//! A `RewriteModule` module pass reaches the raw module token through its
//! mutator and adds a global; because the rung mutates, the driver returns
//! `Module<Unverified>`. The `#[module_pass]` macro expands this to the raw
//! [`ModulePass`](crate::ModulePass) impl — `ModCx<Self>`/`ModReport` in the
//! signature are sentinels the macro rewrites, so they are not imported.
//!
//! ```
//! use llvmkit_ir::{Analyses, IrError, Module, Unverified, module_pass, run_module_pass};
//!
//! struct AddMarkerGlobal;
//!
//! #[module_pass(name = "add-marker-global", access = RewriteModule)]
//! impl AddMarkerGlobal {
//!     fn run(&mut self, cx: ModCx<Self>) -> IrResult<ModReport> {
//!         let rewrite = cx.mutate(); // consumes `cx`; no all-preserved report left
//!         rewrite.module_mut().add_global("marker", 0i32)?; // literal initializer
//!         Ok(rewrite.done()) // RewriteModule floor: nothing preserved
//!     }
//! }
//!
//! fn main() -> Result<(), IrError> {
//!     Module::with_new("mod-pass-doc", |m| {
//!         let verified = m.verify()?;
//!         let mut analyses = Analyses::new();
//!         let rewritten: Module<'_, _, Unverified> =
//!             run_module_pass(AddMarkerGlobal, verified, &mut analyses)?;
//!         assert_eq!(rewritten.iter_globals().len(), 1);
//!         let _ = rewritten.verify()?;
//!         Ok(())
//!     })
//! }
//! ```

#![deny(missing_docs)]

use core::marker::PhantomData;

use super::BasicBlock;
use super::IrResult;
use super::analysis::{
    AnalysisSelector, CfgIncremental, FunctionAnalysis, FunctionAnalysisList,
    FunctionAnalysisManager, ModuleAnalysis, ModuleAnalysisList, ModuleAnalysisManager,
    ModuleAnalysisSelector, PreservedAnalyses, RepairOutcome,
};
use super::basic_block::BasicBlockLabel;
use super::block_state::{Terminated, Unterminated};
use super::cfg_update::CfgUpdate;
use super::dominator_tree::DominatorTreeAnalysis;
use super::error::IrError;
use super::function::FunctionValue;
use super::instruction::{Instruction, InstructionView, NonTerminator, TerminatorKind, state};
use super::ir_builder::IRBuilder;
use super::marker::{Dyn, ReturnMarker};
use super::module::{Brand, Invariant, Module, ModuleBrand, ModuleRef, ModuleView, Unverified};
use super::pass_access::{
    FnAccess, ModAccess, MutatingFn, MutatingModule, PatchBody, ReshapeCfg, RewriteModule,
};
use super::phi_check::{check_phi_incoming, render_phi_violation};
use super::r#type::Type;
use super::value::{IsValue, Value, ValueId};
use super::worklist::Worklist;

/// Read-only view of a basic block under its owning module brand.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct BasicBlockView<'ctx, B: ModuleBrand = Brand<'ctx>> {
    block: BasicBlock<'ctx, Dyn, Terminated, B>,
    _brand: Invariant<B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> Clone for BasicBlockView<'ctx, B> {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            block: self.block.copy_handle(),
            _brand: PhantomData,
        }
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> BasicBlockView<'ctx, B> {
    #[inline]
    pub(super) fn new(block: BasicBlock<'ctx, Dyn, Terminated, B>) -> Self {
        Self {
            block,
            _brand: PhantomData,
        }
    }

    /// Underlying basic-block handle.
    #[inline]
    pub(super) fn as_basic_block(&self) -> BasicBlock<'ctx, Dyn, Terminated, B> {
        self.block.copy_handle()
    }

    /// Optional textual name.
    #[inline]
    pub fn name(&self) -> Option<String> {
        self.block.name()
    }

    /// Parent function if the block is attached.
    #[inline]
    pub fn parent_function(&self) -> Option<FunctionView<'ctx, B>> {
        let id = self.block.parent_id()?;
        Some(FunctionView::new(FunctionValue::from_parts_unchecked(
            id,
            ModuleRef::<B>::new(self.block.module().core_ref()),
        )))
    }

    /// Number of instructions in program order.
    #[inline]
    pub fn instruction_count(&self) -> usize {
        self.block.instructions().len()
    }

    /// Read-only instruction views in program order. Lets an `Inspect`-rung
    /// pass walk a block's instructions without escaping to the underlying
    /// function handle.
    #[inline]
    pub fn instructions(&self) -> impl ExactSizeIterator<Item = InstructionView<'ctx, B>> {
        self.block.instructions()
    }

    /// `true` if the block currently has no instructions.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.block.is_empty()
    }
}

/// Read-only view of a function under its owning module brand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FunctionView<'ctx, B: ModuleBrand = Brand<'ctx>> {
    function: FunctionValue<'ctx, Dyn, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionView<'ctx, B> {
    #[inline]
    pub(super) fn new(function: FunctionValue<'ctx, Dyn, B>) -> Self {
        Self { function }
    }

    /// Underlying typed function handle in erased-return form.
    #[inline]
    pub(super) fn as_function(self) -> FunctionValue<'ctx, Dyn, B> {
        self.function
    }

    /// Function name.
    #[inline]
    pub fn name(self) -> &'ctx str {
        self.function.name()
    }

    /// Owning module.
    #[inline]
    pub fn module(self) -> ModuleView<'ctx, B> {
        self.function.module()
    }

    /// Entry block if the function is a definition.
    #[inline]
    pub fn entry_block(self) -> Option<BasicBlockView<'ctx, B>> {
        self.function.entry_block().map(BasicBlockView::new)
    }

    /// Basic blocks in insertion order.
    #[inline]
    pub fn basic_blocks(self) -> impl ExactSizeIterator<Item = BasicBlockView<'ctx, B>> + 'ctx {
        self.function.basic_blocks().map(BasicBlockView::new)
    }
}

impl<'ctx, R: ReturnMarker, B: ModuleBrand + 'ctx> From<FunctionValue<'ctx, R, B>>
    for FunctionView<'ctx, B>
{
    #[inline]
    fn from(function: FunctionValue<'ctx, R, B>) -> Self {
        Self::new(function.as_dyn())
    }
}

/// Mutation-capable view of one function body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FunctionBody<'ctx, B: ModuleBrand = Brand<'ctx>> {
    function: FunctionValue<'ctx, Dyn, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionBody<'ctx, B> {
    #[inline]
    pub(super) fn new(function: FunctionValue<'ctx, Dyn, B>) -> Self {
        Self { function }
    }

    /// Read-only view of the same function.
    #[inline]
    pub fn as_view(self) -> FunctionView<'ctx, B> {
        FunctionView::new(self.function)
    }

    /// Underlying function handle for body-local mutation APIs.
    #[inline]
    pub fn as_function(self) -> FunctionValue<'ctx, Dyn, B> {
        self.function
    }

    /// Function name.
    #[inline]
    pub fn name(self) -> &'ctx str {
        self.function.name()
    }

    /// Entry block if the function is a definition.
    #[inline]
    pub fn entry_block(self) -> Option<BasicBlock<'ctx, Dyn, Terminated, B>> {
        self.function.entry_block()
    }

    /// Basic blocks in insertion order.
    #[inline]
    pub fn basic_blocks(
        self,
    ) -> impl ExactSizeIterator<Item = BasicBlock<'ctx, Dyn, Terminated, B>> + 'ctx {
        self.function.basic_blocks()
    }
}

/// Iterator over read-only function views in module order.
pub struct ModuleFunctionViews<'ctx, B: ModuleBrand = Brand<'ctx>> {
    inner: Box<dyn ExactSizeIterator<Item = FunctionView<'ctx, B>> + 'ctx>,
}

impl<'ctx, B: ModuleBrand + 'ctx> ModuleFunctionViews<'ctx, B> {
    #[inline]
    pub(super) fn new(module: ModuleView<'ctx, B>) -> Self {
        Self {
            inner: Box::new(module.iter_functions()),
        }
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> Iterator for ModuleFunctionViews<'ctx, B> {
    type Item = FunctionView<'ctx, B>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> ExactSizeIterator for ModuleFunctionViews<'ctx, B> {
    #[inline]
    fn len(&self) -> usize {
        self.inner.len()
    }
}

// ==========================================================================
// capability-graded pass API — function report, entry context, and mutators
// ==========================================================================

/// The value a function pass returns. Wraps the driver-derived
/// [`PreservedAnalyses`]. Its constructors are `pub(crate)`: an external author
/// can *name* the type (it appears in a pass's `run` return) but can only
/// *obtain* one from an [`FnCx`] or a mutator, never fabricate it. That is what
/// makes over-claiming preservation unspellable — the report always carries the
/// floor the consumed capability rung structurally allows, not an author's
/// optimistic guess (D1; D8).
///
/// Unparameterized on purpose: the honesty guarantee comes from the consuming
/// transition ([`FnCx::mutate`] discards the "all preserved" shortcut), not from
/// a type tag. It is a distinct type from the module report ([`ModReport`]), so a
/// function pass cannot return a module report by mistake.
pub struct FnReport {
    pa: PreservedAnalyses,
    /// The reshape mutator's witnessed [`CfgUpdate`] log, carried out to the
    /// driver so its `done()`-flush can offer these edits to cached CFG
    /// analyses (and mark preserved those that repair). Empty for a non-reshape
    /// report. Not a preservation *claim* — the driver still witnesses each
    /// analysis repair before preserving it.
    cfg_updates: Vec<CfgUpdate>,
}

impl FnReport {
    /// Wrap a driver-derived preservation set (no CFG-edit log). `pub(crate)` —
    /// this is a sole construction path, so an external author can never
    /// fabricate a report that over-claims preservation. THE honesty guarantee.
    #[inline]
    pub(crate) fn from_pa(pa: PreservedAnalyses) -> Self {
        Self {
            pa,
            cfg_updates: Vec::new(),
        }
    }

    /// Wrap a preservation set together with the reshape mutator's recorded
    /// [`CfgUpdate`] log. `pub(crate)` — same honesty guarantee as
    /// [`Self::from_pa`]; the log is witnessed, not author-claimed.
    #[inline]
    pub(crate) fn from_pa_with_cfg_updates(
        pa: PreservedAnalyses,
        cfg_updates: Vec<CfgUpdate>,
    ) -> Self {
        Self { pa, cfg_updates }
    }

    /// Consume the report into its preservation set and recorded CFG-edit log.
    /// The function drivers read both: the log drives the `done()`-flush, then
    /// the (possibly-augmented) set drives invalidation. Tests that only want
    /// the set take `.into_parts().0`.
    #[inline]
    pub(crate) fn into_parts(self) -> (PreservedAnalyses, Vec<CfgUpdate>) {
        (self.pa, self.cfg_updates)
    }
}

/// Consuming entry context handed to a function pass at capability rung `A`.
///
/// Parameterized by the access marker `A` (which rung) and the `Requires` list
/// `R` (which analyses were prefetched) rather than by a pass trait, so the
/// context type stands alone. The `FunctionPass` trait spells its `run` signature
/// as `FnCx<'_, '_, 'ctx, B, Self::Access, Self::Requires>`.
///
/// The typestate that makes a preservation lie unspellable: to change the IR a
/// pass must call [`FnCx::mutate`], which **consumes** the context and returns a
/// rung-specific mutator. Before `mutate()`, [`FnCx::unchanged`] yields an
/// all-preserved report; after it, the context is gone, so the only report left
/// is the mutator's `done()` → the rung's derived preservation floor. This is
/// the same consuming-handle discipline the crate already uses for terminated
/// blocks (D1) and erased instructions (D2).
///
/// The module `token` (`'pm`) and the
/// prefetched `results` (`'r`) carry distinct lifetimes: the token borrows the
/// long-lived pipeline module while the results borrow the analysis manager only
/// for the pass's scope. (llvmkit-specific capability-context lock — no upstream
/// analog: LLVM pass contexts are untyped `Function&` + `FAM&`.)
pub struct FnCx<'pm, 'r, 'ctx, B, A, R>
where
    B: ModuleBrand + 'ctx,
    A: FnAccess,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'pm,
    'ctx: 'r,
{
    token: A::Token<'pm, 'ctx, B>,
    function: FunctionView<'ctx, B>,
    results: R::ResultRefs<'r>,
}

impl<'pm, 'r, 'ctx, B, A, R> FnCx<'pm, 'r, 'ctx, B, A, R>
where
    B: ModuleBrand + 'ctx,
    A: FnAccess,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'pm,
    'ctx: 'r,
{
    /// Assemble a context from the driver-prefetched parts. The driver-facing
    /// seam: [`crate::pass_manager::run_function_pass`] (in-crate) and these
    /// tests construct contexts here. `pub(crate)` — the honesty guarantee rests
    /// on [`FnReport::from_pa`] being non-public, not on this constructor, and
    /// the single-pass driver is now its sole non-test caller.
    #[inline]
    pub(crate) fn new(
        token: A::Token<'pm, 'ctx, B>,
        function: FunctionView<'ctx, B>,
        results: R::ResultRefs<'r>,
    ) -> Self {
        Self {
            token,
            function,
            results,
        }
    }

    /// Read-only function view.
    #[inline]
    pub fn function(&self) -> FunctionView<'ctx, B> {
        self.function
    }

    /// Owning module view.
    #[inline]
    pub fn module(&self) -> ModuleView<'ctx, B> {
        self.function.module()
    }

    /// Infallible access to a `Requires`-declared analysis result. The position
    /// index `I` is inferred; an undeclared analysis has no [`AnalysisSelector`]
    /// impl and fails to compile.
    #[inline]
    pub fn analysis<A2, I>(&self) -> &'r A2::Result
    where
        A2: FunctionAnalysis<'ctx, B>,
        R: AnalysisSelector<'ctx, B, A2, I>,
    {
        R::select(&self.results)
    }

    /// Finish without mutating: report everything preserved. Available at every
    /// rung ("I inspected / changed nothing"). Consumes the context.
    #[inline]
    pub fn done(self) -> FnReport {
        FnReport::from_pa(PreservedAnalyses::all())
    }
}

impl<'pm, 'r, 'ctx, B, A, R> FnCx<'pm, 'r, 'ctx, B, A, R>
where
    B: ModuleBrand + 'ctx,
    A: MutatingFn,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'pm,
    'ctx: 'r,
{
    /// The didn't-actually-mutate shortcut: report everything preserved without
    /// entering the mutator. Consumes the context, so it cannot be paired with a
    /// later `mutate()`.
    #[inline]
    pub fn unchanged(self) -> FnReport {
        FnReport::from_pa(PreservedAnalyses::all())
    }

    /// Transition into mutation: **consumes** the context and moves its token,
    /// function, and prefetched results into the rung's mutator. Once called,
    /// `unchanged()`/`done()` on the context are unspellable — the only report
    /// left is the mutator's `done()`, which carries the rung's preservation
    /// floor. This is the core honesty mechanism.
    #[inline]
    pub fn mutate(self) -> <A as MutatingFn>::Mutator<'pm, 'r, 'ctx, B, R> {
        A::into_mutator(self.token, self.function, self.results)
    }
}

/// Instruction-level mutator for the [`PatchBody`] rung — the workhorse. Edits
/// instructions within existing blocks; it has **no** terminator or CFG method,
/// which is exactly what makes its `done()` floor ("CFG analyses preserved")
/// sound by construction. Mutation flows through the shared
/// `&Module<Unverified>` token via interior mutability (never `&mut Module`),
/// the same discipline `DcePass` uses.
///
/// Carries the prefetched analysis results, so a transform can read analyses
/// *while* it edits (the results borrow the analysis manager; mutation borrows
/// the module token — distinct objects, no aliasing).
pub struct FnPatch<'m, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    module: &'m Module<'ctx, B, Unverified>,
    function: FunctionView<'ctx, B>,
    results: R::ResultRefs<'r>,
    /// Witnessed dirty flag: set by every mutating method, read by
    /// [`Self::done`]. A run that touches nothing reports everything
    /// preserved; a run that mutates reports the rung floor. This is a
    /// *fact the mutator observes*, not a claim the author makes — a `Cell`
    /// so mutating methods can flip it through a shared `&self` (mutation
    /// itself flows through the interior-mutable module token).
    dirty: core::cell::Cell<bool>,
    /// Opt-in instruction worklist. When `Some`, [`Self::erase`] /
    /// [`Self::replace_all_uses`] maintain it (push cascade + self-remove), so a
    /// worklist pass reaches a fixpoint without a restart-scan. `None` (the
    /// default) is exactly today's behavior — no overhead, no behavior change.
    worklist: core::cell::RefCell<Option<Worklist>>,
}

impl<'m, 'r, 'ctx, B, R> FnPatch<'m, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    #[inline]
    pub(crate) fn new(
        module: &'m Module<'ctx, B, Unverified>,
        function: FunctionView<'ctx, B>,
        results: R::ResultRefs<'r>,
    ) -> Self {
        Self {
            module,
            function,
            results,
            dirty: core::cell::Cell::new(false),
            worklist: core::cell::RefCell::new(None),
        }
    }

    /// Whether any mutation has been performed through this mutator.
    #[inline]
    pub fn is_dirty(&self) -> bool {
        self.dirty.get()
    }

    /// Read-only function view.
    #[inline]
    pub fn function(&self) -> FunctionView<'ctx, B> {
        self.function
    }

    /// Mutation-capable function-body view.
    #[inline]
    pub fn function_mut(&self) -> FunctionBody<'ctx, B> {
        FunctionBody::new(self.function.as_function())
    }

    /// Infallible access to a `Requires`-declared analysis result *during*
    /// mutation. The results borrow the analysis manager; mutation goes through
    /// the module token — different objects, no aliasing.
    #[inline]
    pub fn analysis<A2, I>(&self) -> &'r A2::Result
    where
        A2: FunctionAnalysis<'ctx, B>,
        R: AnalysisSelector<'ctx, B, A2, I>,
    {
        R::select(&self.results)
    }

    /// Mutation-capable module token for saved-handle mutators / the IR builder.
    #[inline]
    pub fn module_mut(&self) -> &'m Module<'ctx, B, Unverified> {
        self.module
    }

    /// Erase a non-terminator instruction from its parent block, deregistering
    /// its operand uses. Mirrors `DcePass`'s erase step
    /// ([`Instruction::erase_from_parent`]).
    ///
    /// Accepts only a [`NonTerminator`] — obtained from
    /// [`InstructionView::as_non_terminator`] — so a terminator-erase (which
    /// would break the "CFG preserved" floor this rung rests on) is a compile
    /// error, not a runtime rejection. Infallible: erasing a non-terminator
    /// cannot fail.
    #[inline]
    pub fn erase(&self, target: &NonTerminator<'ctx, B>) {
        let id = target.as_value().id();
        let inst = Instruction::<state::Attached, B>::from_parts(id, self.module.module_ref());
        // Capture operand ids before erasing (erase drops their uses). Push them
        // all unconditionally — `Worklist::pop` is panic-safe and skips any id that
        // is not an instruction (constant/param operands), so no filter is needed
        // here. Then remove `id` itself so the erased instruction never surfaces.
        if let Some(wl) = self.worklist.borrow_mut().as_mut() {
            for op_id in inst.as_view().operand_ids() {
                wl.push(op_id);
            }
            wl.remove(id);
        }
        inst.erase_from_parent(self.module);
        self.dirty.set(true);
    }

    /// Early-increment walk over every non-terminator of the function body, in
    /// program order. Each block's instruction ids are snapshotted up front and
    /// walked by index, so erasing the *yielded* instruction does not disturb the
    /// walk (its successor is already fixed — LLVM's `make_early_inc_range`
    /// idiom). Yields [`NonTerminator`] (so [`Self::erase`] takes it directly) and
    /// never yields a terminator. Cascades (erasing instructions *ahead* of the
    /// cursor) are a worklist's job, not the cursor's.
    #[inline]
    pub fn body_instructions(&self) -> impl Iterator<Item = NonTerminator<'ctx, B>> + '_ {
        let module = self.module.module_ref();
        self.function
            .as_function()
            .basic_blocks()
            .flat_map(move |block| block.instruction_ids())
            .filter_map(move |id| InstructionView::from_parts(id, module).as_non_terminator())
    }

    /// Replace every use of `view`'s result with `replacement`, leaving the
    /// instruction itself in place. Mirrors
    /// [`Instruction::replace_all_uses_with`].
    #[inline]
    pub fn replace_all_uses<V>(
        &self,
        view: &InstructionView<'ctx, B>,
        replacement: V,
    ) -> IrResult<()>
    where
        V: IsValue<'ctx, B>,
    {
        let id = view.as_value().id();
        // Capture the former users only when a worklist is active — the
        // inactive path must stay allocation-free (the field's zero-overhead
        // promise). The `borrow()` is a let-RHS temporary, released before the
        // later `borrow_mut()`. Users must be captured *before* the RAUW rewires
        // them.
        let users: Vec<ValueId> = if self.worklist.borrow().is_some() {
            view.as_value().users().map(|u| u.as_value().id).collect()
        } else {
            Vec::new()
        };
        let inst = Instruction::<state::Attached, B>::from_parts(id, self.module.module_ref());
        inst.replace_all_uses_with(self.module, replacement)?;
        if let Some(wl) = self.worklist.borrow_mut().as_mut() {
            for user_id in users {
                wl.push(user_id);
            }
        }
        self.dirty.set(true);
        Ok(())
    }

    /// Begin a worklist-driven fixpoint transform: activate a [`Worklist`] on this
    /// mutator, seeded with every non-terminator of the function body in program
    /// order. Drive it with `while let Some(inst) = scope.next() { ... }`, mutating
    /// through `self` (`erase`/`replace_all_uses`) — those mutations maintain the
    /// worklist automatically (cascade + self-remove). The worklist deactivates
    /// when the returned scope drops.
    ///
    /// Only one [`WorklistScope`] may be active on a given `FnPatch` at a time;
    /// creating a second while one is live panics.
    pub fn worklist(&self) -> WorklistScope<'_, 'm, 'r, 'ctx, B, R> {
        // Reject re-entry: the shared slot holds one worklist, and either
        // scope's `Drop` resets it to `None`, so a nested scope would silently
        // reseed and leave the outer scope inert. Fail loudly instead. The
        // `borrow()` is a temporary released before the `borrow_mut()` below;
        // `body_instructions()` does not touch `self.worklist`, so no overlap.
        assert!(
            self.worklist.borrow().is_none(),
            "a WorklistScope is already active on this FnPatch; nested worklist scopes are not supported"
        );
        let mut wl = Worklist::new();
        for inst in self.body_instructions() {
            wl.push(inst.as_value().id());
        }
        *self.worklist.borrow_mut() = Some(wl);
        WorklistScope { patch: self }
    }

    /// Finish: report the [`PatchBody`] preservation floor (CFG analyses
    /// preserved) if anything was mutated, or everything-preserved if the
    /// run was a no-op. Consumes the mutator. The all-preserved case is
    /// *witnessed* by the dirty flag, so it needs no read-only pre-scan.
    #[inline]
    pub fn done(self) -> FnReport {
        if self.dirty.get() {
            FnReport::from_pa(<PatchBody as FnAccess>::preserved_floor())
        } else {
            FnReport::from_pa(PreservedAnalyses::all())
        }
    }
}

/// RAII handle activating a [`Worklist`] on an [`FnPatch`] for the duration of
/// a fixpoint transform. Created by [`FnPatch::worklist`]: it seeds the
/// worklist with every non-terminator of the function body and, on drop,
/// deactivates it. [`Self::next`] pops the next instruction to process; the
/// pass mutates through the `FnPatch` directly, and those mutations maintain
/// the worklist (push cascade, self-remove) automatically.
pub struct WorklistScope<'p, 'm, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    patch: &'p FnPatch<'m, 'r, 'ctx, B, R>,
}

impl<'p, 'm, 'r, 'ctx, B, R> WorklistScope<'p, 'm, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    /// Pop the next instruction to process, or `None` when the fixpoint is
    /// reached. Skips terminators and erased ids (the latter never surface —
    /// `erase` removes them).
    #[inline]
    pub fn next(&self) -> Option<NonTerminator<'ctx, B>> {
        let module = self.patch.module.module_ref();
        self.patch.worklist.borrow_mut().as_mut()?.pop(module)
    }
}

impl<'p, 'm, 'r, 'ctx, B, R> Drop for WorklistScope<'p, 'm, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    #[inline]
    fn drop(&mut self) {
        *self.patch.worklist.borrow_mut() = None;
    }
}

/// Which single successor slot of a `switch`/`br`/`cond_br` terminator an edge
/// edit acts on. Fixed by the typed terminator-edit handles (e.g.
/// [`SwitchEdit::redirect_successor`], [`CondBrEdit::remove_then`]) — each hands
/// a constant slot to the mechanical mutation helpers
/// ([`FnReshape::remove_slot`]/[`FnReshape::redirect_slot`]).
///
/// The `br`/`cond_br` slots are *role-based* (which arm), so the helper reads
/// the old target off the current terminator; the switch case slot is
/// *target-based* — it carries the old destination id because a switch edit
/// touches *every* case pointing at that block. The `invoke`/`callbr` slots are
/// role-based too, and — unlike the switch/branch slots — appear only in
/// [`FnReshape::redirect_slot`]: an `invoke`/`callbr` edge is not removable, so
/// there is deliberately no [`FnReshape::remove_slot`] arm for them (the absence
/// is the mechanical-layer half of the "no remove" guarantee the typed handles
/// enforce at compile time).
#[derive(Clone, Copy)]
enum EditSlot {
    /// Every `switch` case whose destination is the carried id.
    SwitchCase(ValueId),
    /// The `switch` default edge.
    SwitchDefault,
    /// The `then` arm of a `cond_br`.
    BrThen,
    /// The `else` arm of a `cond_br`.
    BrElse,
    /// The sole target of an unconditional `br`.
    BrUncond,
    /// The normal (non-unwind) destination of an `invoke`.
    InvokeNormal,
    /// The unwind destination of an `invoke`.
    InvokeUnwind,
    /// The default (fallthrough) destination of a `callbr`.
    CallBrDefault,
    /// The `i`-th indirect destination of a `callbr`.
    CallBrIndirect(usize),
}

/// CFG-rewriting mutator for the [`ReshapeCfg`] rung — minimal but real. It has
/// everything [`FnPatch`] exposes (by composition) **plus** at least one genuine
/// control-flow operation ([`FnReshape::split_block`], wired to
/// [`BasicBlock::split_at`]), so the rung is distinct from `FnPatch` and its
/// `done()` floor is `none()` — nothing preserved.
///
/// Structural ops maintain the phis of every block they touch as part of the op
/// itself (split_block rewrites successor incomings; there is no separate fixup
/// call to forget). Ops that make phis *gain* entries take the new values as
/// typed arguments and witness every SSA obligation at the call:
/// [`FnReshape::insert_phi`] creates a phi that is complete against `block`'s
/// predecessors, correctly typed, free of differing duplicates, and whose every
/// incoming value dominates its edge — all checkable because a `ReshapeCfg` pass
/// sees the whole CFG. A general in-pass `IRBuilder` surface and fuller
/// terminator surgery — rewiring branches, deleting blocks — remain future work.
pub struct FnReshape<'m, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    patch: FnPatch<'m, 'r, 'ctx, B, R>,
    /// Witnessed CFG-edit log: every structural edit method appends its own
    /// [`CfgUpdate`] decomposition here as it runs. The driver drains this at
    /// `done()` (and a future mid-pass repair reads it) to offer each cached
    /// CFG analysis exactly the edits it must absorb — so preservation is
    /// *observed*, never author-claimed. A [`RefCell`](core::cell::RefCell) for
    /// the same reason [`FnPatch`]'s `dirty` flag is a `Cell`: the recording
    /// edit methods append through a shared `&self` while IR mutation flows
    /// through the interior-mutable module token.
    cfg_updates: core::cell::RefCell<Vec<CfgUpdate>>,
    /// Graveyard of freshly-repaired/recomputed analysis results produced by
    /// [`Self::analysis_repaired`]. Each mid-pass repair pushes its owned result
    /// here and hands back a borrow into it whose lifetime is tied to the
    /// `&mut self` receiver — so holding a repaired CFG-analysis reference across
    /// a later structural edit is a *compile error*, and the stale-read footgun
    /// is unrepresentable rather than merely discouraged. Grows by one entry per
    /// `analysis_repaired` call for the mutator's lifetime (a pass runs finitely
    /// many repairs).
    repaired: Vec<Box<dyn core::any::Any>>,
}

impl<'m, 'r, 'ctx, B, R> FnReshape<'m, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    #[inline]
    pub(crate) fn new(
        module: &'m Module<'ctx, B, Unverified>,
        function: FunctionView<'ctx, B>,
        results: R::ResultRefs<'r>,
    ) -> Self {
        Self {
            patch: FnPatch::new(module, function, results),
            cfg_updates: core::cell::RefCell::new(Vec::new()),
            repaired: Vec::new(),
        }
    }

    /// The CFG edits recorded by structural edit methods so far, in the order
    /// they were performed. Each [`CfgUpdate`] was minted by the mutator itself
    /// as it edited — the log is a *witnessed* fact, not an author claim. The
    /// driver drains this to decide which cached CFG analyses it can mark
    /// preserved (via the [`CfgIncremental`](crate::analysis) hook); exposed
    /// here so tests and the driver can inspect it.
    #[inline]
    pub fn pending_cfg_updates(&self) -> Vec<CfgUpdate> {
        self.cfg_updates.borrow().clone()
    }

    // The in-block-edit surface below is delegated to the inner `FnPatch` by
    // hand rather than through `Deref`. `Deref<Target = FnPatch>` would also
    // expose `FnPatch::analysis` — whose `&'r`-borrowed reference outlives a
    // reshape edit — so a pass could read a CFG analysis, split a block, then
    // read the now-stale cached reference. Withholding that method (and offering
    // only [`Self::analysis_repaired`], whose result is tied to `&mut self`) is
    // what makes the mid-reshape stale-read *unrepresentable*. `erase`/
    // `replace_all_uses` are safe to expose: an in-block edit preserves the CFG,
    // so it cannot invalidate a CFG analysis.

    /// Read-only function view. Delegated from the inner [`FnPatch`].
    #[inline]
    pub fn function(&self) -> FunctionView<'ctx, B> {
        self.patch.function()
    }

    /// Mutation-capable function-body view. Delegated from the inner [`FnPatch`].
    #[inline]
    pub fn function_mut(&self) -> FunctionBody<'ctx, B> {
        self.patch.function_mut()
    }

    /// Whether any mutation has been performed through this mutator (including
    /// its inner [`FnPatch`] surface). Delegated from the inner [`FnPatch`].
    #[inline]
    pub fn is_dirty(&self) -> bool {
        self.patch.is_dirty()
    }

    /// Mutation-capable module token. Delegated from the inner [`FnPatch`].
    #[inline]
    pub fn module_mut(&self) -> &'m Module<'ctx, B, Unverified> {
        self.patch.module_mut()
    }

    /// Erase a non-terminator instruction. Delegated from the inner [`FnPatch`];
    /// an in-block erase preserves the CFG, so it records no [`CfgUpdate`].
    #[inline]
    pub fn erase(&self, target: &NonTerminator<'ctx, B>) {
        self.patch.erase(target);
    }

    /// Replace every use of `view`'s result with `replacement`. Delegated from
    /// the inner [`FnPatch`]; preserves the CFG.
    #[inline]
    pub fn replace_all_uses<V>(
        &self,
        view: &InstructionView<'ctx, B>,
        replacement: V,
    ) -> IrResult<()>
    where
        V: IsValue<'ctx, B>,
    {
        self.patch.replace_all_uses(view, replacement)
    }

    /// Read a `Requires`-declared **CFG analysis** mid-reshape, brought up to
    /// date with every structural edit recorded so far.
    ///
    /// This is the *only* way to read a CFG analysis on a reshape mutator, and
    /// it is what makes the stale-read footgun unrepresentable. The returned
    /// reference borrows `&mut self`, so the borrow checker forbids holding it
    /// across any later mutator call (a structural edit, another repair, even an
    /// erase): a stale read cannot be written down. To read again after an edit,
    /// call this again — it re-derives from the freshly recorded edits.
    ///
    /// Mechanism: the recorded [`CfgUpdate`]s are drained and offered to a
    /// working copy of the cached result through
    /// [`CfgIncremental::apply_updates`]. If the analysis absorbs them
    /// ([`RepairOutcome::Repaired`]) that copy is used; otherwise
    /// ([`RepairOutcome::PreferRecompute`]) the result is recomputed from
    /// scratch. Either way the fresh result is stored in the
    /// mutator and a borrow into it is returned. The framework *witnesses* the
    /// repair; the author never claims it.
    ///
    /// The bound `A::Result: CfgIncremental` is precisely the "this is a CFG
    /// analysis" marker: value analyses (which a reshape edit cannot make stale)
    /// have no such hook and are not read through here.
    #[inline]
    pub fn analysis_repaired<A, I>(&mut self) -> &A::Result
    where
        A: FunctionAnalysis<'ctx, B>,
        A::Result: CfgIncremental<'ctx, B> + Clone,
        R: AnalysisSelector<'ctx, B, A, I>,
    {
        // Snapshot (do NOT drain) the recorded edits: the log must survive for
        // the driver's `done()`-flush, which repairs the FAM-cached result the
        // same way. Recompute-based repair makes reading a snapshot each time
        // correct regardless of call order.
        let updates: Vec<CfgUpdate> = self.cfg_updates.get_mut().clone();
        let function = self.patch.function();
        // Offer the recorded edits to a working copy of the cached result; fall
        // back to a from-scratch recompute when the analysis declines them.
        let mut working = R::select(&self.patch.results).clone();
        let fresh = match working.apply_updates(&updates, function) {
            RepairOutcome::Repaired => working,
            RepairOutcome::PreferRecompute => {
                <A::Result as CfgIncremental<'ctx, B>>::recompute(function)
            }
        };
        self.repaired.push(Box::new(fresh));
        self.repaired
            .last()
            .expect("just pushed a result")
            .downcast_ref::<A::Result>()
            .expect("pushed value is A::Result")
    }

    /// Split `block` before instruction `before`: `before` and everything after
    /// it move into a fresh block (named `name`) appended to the function; the
    /// original block keeps the prefix. The caller is responsible for adding a
    /// terminator flowing to the new block. The genuine CFG operation that makes
    /// this rung distinct from [`FnPatch`]; wired to [`BasicBlock::split_at`].
    #[inline]
    pub fn split_block<Name>(
        &self,
        block: &BasicBlockView<'ctx, B>,
        before: &InstructionView<'ctx, B>,
        name: Name,
    ) -> IrResult<BasicBlock<'ctx, Dyn, Unterminated, B>>
    where
        Name: Into<String>,
    {
        // Capture the successors of `block`'s terminator *before* the split
        // moves that terminator into the new block — afterwards `block` is
        // unterminated and has none. The split's own effect on the CFG is
        // purely this rewiring: each edge `block → s` becomes `new_block → s`
        // (the caller wires the fresh `block → new_block` edge later, through
        // its own terminator, so that edge is not this method's to record).
        let source = block.as_basic_block();
        let source_id = source.as_value().id;
        let successors = crate::cfg::block_successors(&source);

        let new_block = source.split_at(self.patch.module_mut(), before, name)?;
        let new_id = new_block.as_value().id;

        // The terminator moved to `new_block`, so every edge that used to
        // leave `block` now leaves `new_block`. Phis in the successors
        // still name `block`; rewrite them here, as part of the mutation
        // itself — the op carries its own phi maintenance, there is no
        // separate fixup call to forget (mirrors what
        // BasicBlock::replacePhiUsesWith does for upstream splitters).
        for succ in &successors {
            let succ_block: BasicBlock<'ctx, Dyn, Terminated, B> =
                BasicBlock::from_parts(succ.id, succ.module, succ.ty);
            for inst_id in succ_block.instruction_ids() {
                let data = succ.module.value_data(inst_id);
                let crate::value::ValueKindData::Instruction(inst) = &data.kind else {
                    continue;
                };
                let crate::instruction::InstructionKindData::Phi(p) = &inst.kind else {
                    // Phis are grouped at the block top; stop at the first
                    // non-phi instead of scanning the whole block.
                    break;
                };
                for pair in p.incoming.borrow_mut().iter_mut() {
                    if pair.1 == source_id {
                        pair.1 = new_id;
                    }
                }
            }
        }

        if !successors.is_empty() {
            let mut log = self.cfg_updates.borrow_mut();
            for succ in &successors {
                let succ_id = succ.as_value().id;
                log.push(CfgUpdate::delete(source_id, succ_id));
                log.push(CfgUpdate::insert(new_id, succ_id));
            }
        }
        self.patch.dirty.set(true);
        Ok(new_block)
    }

    /// Drop `block`'s leading-phi incomings whose predecessor is `pred_id`,
    /// retaining exactly `keep` of them per phi and deregistering the phi from
    /// each removed value's use-list so reverse-use bookkeeping (and hence RAUW)
    /// stays correct. `keep` is the number of `pred_id → block` edges that
    /// *survive* the terminator edit, so each head phi must end with exactly
    /// `keep` incomings from `pred_id`. Phis are grouped at the block top, so the
    /// scan stops at the first non-phi.
    ///
    /// `keep == 0` is the full LLVM `BasicBlock::removePredecessor`: `pred_id` no
    /// longer reaches `block`, so every `(value, pred_id)` pair leaves each head
    /// phi, and a phi that thereby loses its *last* incoming is RAUW'd with
    /// poison (of its own result type) and erased, since `%p = phi i32` with no
    /// `[ … ]` pairs has no legal textual form.
    ///
    /// `keep > 0` is the surviving-parallel-edge case: a terminator edit removed
    /// some but not all of the `pred_id → block` edges (one arm of a
    /// `cond_br %c, X, X`; one `switch` case when the default also targets that
    /// block), so `pred_id` still reaches `block`. Only the excess incomings
    /// beyond the first `keep` are dropped and no phi is erased — a phi that
    /// retains an incoming has a legal textual form. A verified module carries
    /// the same value for every incoming of a given predecessor (differing
    /// duplicates for one block are a coherence error), so *which* `keep` copies
    /// remain is immaterial.
    ///
    /// Two passes, collect-then-mutate: pass 1 does the `retain` +
    /// use-list-deregister and records which phis emptied; pass 2 RAUWs and
    /// erases them. The split is required — RAUW/erase re-enter the phi's
    /// storage and the block instruction list, so they cannot run while pass 1
    /// holds those borrows or walks the (owned) id snapshot. An emptied phi has
    /// zero incomings, so it references no other emptied phi: the pass-2 order is
    /// immaterial.
    fn drop_incoming_from_pred(
        &self,
        block: &BasicBlock<'ctx, Dyn, Terminated, B>,
        pred_id: ValueId,
        keep: usize,
    ) -> IrResult<()> {
        let ctx = self.patch.module_mut().core_ref().context();
        // Pass 1: drop `pred_id`'s incomings beyond the first `keep`, collecting
        // phis left with none.
        let mut emptied: Vec<ValueId> = Vec::new();
        for inst_id in block.instruction_ids() {
            let crate::value::ValueKindData::Instruction(inst) = &ctx.value_data(inst_id).kind
            else {
                continue;
            };
            let crate::instruction::InstructionKindData::Phi(p) = &inst.kind else {
                break;
            };
            let mut dropped_values: Vec<ValueId> = Vec::new();
            {
                // Retain the first `keep` incomings from `pred_id` (one per
                // surviving parallel edge); drop the rest.
                let mut kept = 0_usize;
                let mut incoming = p.incoming.borrow_mut();
                incoming.retain(|(value, pred)| {
                    if *pred == pred_id {
                        if kept < keep {
                            kept += 1;
                            true
                        } else {
                            dropped_values.push(value.get());
                            false
                        }
                    } else {
                        true
                    }
                });
            }
            for val_id in dropped_values {
                let mut uses = ctx.value_data(val_id).use_list.borrow_mut();
                if let Some(pos) = uses
                    .iter()
                    .position(|e| *e == crate::value::ValueUse::Instruction(inst_id))
                {
                    uses.remove(pos);
                }
            }
            // Release this read borrow immediately — it must not be held into
            // pass 2, whose RAUW/erase re-enter the same phi storage. Only a
            // fully-emptied phi (`keep == 0` and `pred_id` was its last
            // predecessor) is collected; a `keep > 0` phi retains an incoming
            // and never reaches here.
            if p.incoming.borrow().is_empty() {
                emptied.push(inst_id);
            }
        }

        // Pass 2: an emptied phi is unprintable, so replace its uses with poison
        // (of its own type, so RAUW cannot type-mismatch) and detach it. RAUW
        // first (redirect users), erase second (unlink from the block).
        let module = self.patch.module_mut().module_ref();
        for inst_id in emptied {
            let view = crate::instruction::InstructionView::from_parts(inst_id, module);
            let poison = view.ty().get_poison();
            self.replace_all_uses(&view, poison)?;
            self.erase(&view.as_non_terminator().expect("a phi is a non-terminator"));
        }
        Ok(())
    }

    /// Mechanical body of the typed handles' edge-removal ops
    /// ([`CondBrEdit::remove_then`]/[`CondBrEdit::remove_else`]/
    /// [`SwitchEdit::remove_successor`]): perform the terminator mutation
    /// selected by `slot`, drop the removed target's `from`-incoming phis, log
    /// the [`CfgUpdate::delete`], and mark dirty. The edge preconditions are
    /// witnessed by the handle before the slot is built, so this helper does no
    /// validation; a `SwitchDefault`/`BrUncond` slot is unremovable (no handle
    /// method builds one) and never reaches here.
    ///
    /// A `SwitchCase(to)` drops every case targeting `to` (deregistering each
    /// dropped case value's use of the switch); a `BrThen`/`BrElse` collapses
    /// the `cond_br` to an unconditional `br` to the surviving arm and
    /// deregisters the now-dead condition operand. The removed target id — the
    /// block whose `from`-incoming phis are dropped and the delete-edge sink —
    /// is `to` for the switch case and the named arm's current destination for
    /// the branch, read off the terminator before the collapse.
    fn remove_slot(
        &self,
        from: &BasicBlockView<'ctx, B>,
        term_id: ValueId,
        slot: EditSlot,
    ) -> IrResult<()> {
        let from_block = from.as_basic_block();
        let from_id = from_block.as_value().id;
        let ctx = self.patch.module_mut().core_ref().context();

        // Mutate the terminator and learn the removed target block.
        let target_id = match slot {
            EditSlot::SwitchCase(to_id) => {
                let crate::value::ValueKindData::Instruction(i) = &ctx.value_data(term_id).kind
                else {
                    unreachable!("remove_slot: terminator is not an instruction");
                };
                let crate::instruction::InstructionKindData::Switch(switch) = &i.kind else {
                    unreachable!("remove_slot: `SwitchCase` slot on a non-switch terminator");
                };
                // Drop every case targeting `to`, deregistering the switch from
                // each dropped case-value's use-list (the case value is an SSA
                // operand of the switch).
                let mut dropped_case_values: Vec<ValueId> = Vec::new();
                switch.cases.borrow_mut().retain(|(case_val, dest)| {
                    if *dest == to_id {
                        dropped_case_values.push(case_val.get());
                        false
                    } else {
                        true
                    }
                });
                for val_id in dropped_case_values {
                    let mut uses = ctx.value_data(val_id).use_list.borrow_mut();
                    if let Some(pos) = uses
                        .iter()
                        .position(|e| *e == crate::value::ValueUse::Instruction(term_id))
                    {
                        uses.remove(pos);
                    }
                }
                to_id
            }
            EditSlot::BrThen | EditSlot::BrElse => {
                let crate::value::ValueKindData::Instruction(i) = &ctx.value_data(term_id).kind
                else {
                    unreachable!("remove_slot: terminator is not an instruction");
                };
                let crate::instruction::InstructionKindData::Br(branch) = &i.kind else {
                    unreachable!("remove_slot: branch-arm slot on a non-branch terminator");
                };
                // Collapse the `cond_br` to an unconditional `br` to the
                // surviving arm; the removed arm's target is the drop/delete
                // sink and the dead condition operand is deregistered.
                let (cond_id, removed, surviving) = {
                    let kind = branch.kind.borrow();
                    let crate::instr_types::BranchKind::Conditional {
                        cond,
                        then_bb,
                        else_bb,
                    } = &*kind
                    else {
                        unreachable!("remove_slot: branch-arm slot on an unconditional `br`");
                    };
                    match slot {
                        EditSlot::BrThen => (cond.get(), *then_bb, *else_bb),
                        EditSlot::BrElse => (cond.get(), *else_bb, *then_bb),
                        _ => unreachable!("remove_slot: outer arm restricts slot to a branch arm"),
                    }
                };
                *branch.kind.borrow_mut() =
                    crate::instr_types::BranchKind::Unconditional(surviving);
                let mut uses = ctx.value_data(cond_id).use_list.borrow_mut();
                if let Some(pos) = uses
                    .iter()
                    .position(|e| *e == crate::value::ValueUse::Instruction(term_id))
                {
                    uses.remove(pos);
                }
                removed
            }
            EditSlot::SwitchDefault
            | EditSlot::BrUncond
            | EditSlot::InvokeNormal
            | EditSlot::InvokeUnwind
            | EditSlot::CallBrDefault
            | EditSlot::CallBrIndirect(_) => {
                unreachable!("remove_slot: slot is not removable")
            }
        };

        // Mechanically drop the removed target's phi incomings that named
        // `from`, but keep one per surviving parallel `from → target` edge.
        // Removing one arm of a `cond_br %c, X, X` collapses to `br X`, so
        // `from` STILL reaches `X` through the surviving arm; the survivor count
        // is read off the now-mutated terminator (0 for the normal single-edge
        // removal, 1 when the sibling arm targeted the same block). All blocks
        // share the module's label type, so `from`'s block carries the same
        // module ref and label type as the target.
        let target_block = BasicBlock::<Dyn, Terminated, B>::from_parts(
            target_id,
            from_block.module,
            from_block.ty,
        );
        let surviving = crate::cfg::block_successors(&from_block)
            .iter()
            .filter(|succ| succ.as_value().id == target_id)
            .count();
        self.drop_incoming_from_pred(&target_block, from_id, surviving)?;

        self.cfg_updates
            .borrow_mut()
            .push(CfgUpdate::delete(from_id, target_id));
        self.patch.dirty.set(true);
        Ok(())
    }

    /// Mechanical body of every typed terminator-edit handle's `redirect_*`:
    /// reject `from` already reaching
    /// `new_to`, validate `phi_values` against `new_to`'s leading phis,
    /// retarget the single edge selected by `slot` onto `new_to`, drop the old
    /// target's `from`-incomings, seed `new_to`'s phis, log the [`CfgUpdate`]
    /// pair, and mark dirty.
    ///
    /// This is the single validated mutation path, so both the "`from` already
    /// reaches `new_to`" edge precondition and the `phi_values` arity/type
    /// check live here (the former first, preserving the
    /// edge-precondition-before-phi-arity error priority); every typed redirect
    /// handle inherits them for
    /// free. Both are checked *before* any mutation, so a rejected call leaves
    /// the terminator and every phi untouched. The remaining "find-the-slot"
    /// precondition (that `old_to` names a live case) is witnessed by the
    /// target-based [`SwitchEdit::redirect_successor`] via
    /// [`Self::switch_has_case_successor`]; the role-named ops read their arm
    /// off the current terminator and need no such lookup.
    ///
    /// For the role slots (`BrThen`/`BrElse`/`BrUncond`/`InvokeNormal`/
    /// `InvokeUnwind`/`CallBrDefault`/`CallBrIndirect`) the old target is read
    /// off the current terminator; for `SwitchCase(old)` it is carried in the
    /// slot.
    fn redirect_slot(
        &self,
        from: &BasicBlockView<'ctx, B>,
        term_id: ValueId,
        slot: EditSlot,
        new_to: &BasicBlockLabel<'ctx, Dyn, B>,
        phi_values: &[Value<'ctx, B>],
    ) -> IrResult<()> {
        let from_block = from.as_basic_block();
        let from_id = from_block.as_value().id;
        let new_id = new_to.as_value().id;
        let ctx = self.patch.module_mut().core_ref().context();

        // Centralized edge precondition: `from` must not already reach
        // `new_to`. Redirect mints a fresh edge, so each new-target phi gains
        // exactly one `(value, from)` incoming; if `from` already reached
        // `new_to`, seeding would add a duplicate for the same predecessor.
        // Checked here (over the terminator's live successor set, which covers
        // every terminator kind uniformly) and BEFORE the phi-values check, so
        // every redirect path — legacy and typed — shares it at the same error
        // priority.
        if crate::cfg::block_successors(&from_block)
            .iter()
            .any(|succ| succ.as_value().id == new_id)
        {
            return Err(IrError::InvalidOperation {
                message: "redirect: `from` already reaches `new_to`",
            });
        }

        // Collect `new_to`'s leading phis (its block parameters) and validate
        // `phi_values` against them BEFORE mutating — all-or-nothing, mirroring
        // the block-argument branch builder.
        let new_block =
            BasicBlock::<Dyn, Terminated, B>::from_parts(new_to.id, new_to.module, new_to.ty);
        let mut param_phis: Vec<ValueId> = Vec::new();
        for inst_id in new_block.instruction_ids() {
            let crate::value::ValueKindData::Instruction(inst) = &ctx.value_data(inst_id).kind
            else {
                continue;
            };
            let crate::instruction::InstructionKindData::Phi(_) = &inst.kind else {
                break;
            };
            param_phis.push(inst_id);
        }
        if param_phis.len() != phi_values.len() {
            return Err(IrError::PhiArgArityMismatch {
                expected: param_phis.len(),
                got: phi_values.len(),
            });
        }
        for (phi_id, value) in param_phis.iter().zip(phi_values.iter()) {
            let phi_ty = ctx.value_data(*phi_id).ty;
            if value.ty != phi_ty {
                return Err(IrError::TypeMismatch {
                    expected: Type::new(phi_ty, new_to.module).kind_label(),
                    got: Type::new(value.ty, new_to.module).kind_label(),
                });
            }
        }

        // All preconditions witnessed -- retarget the single edge onto
        // `new_to`, reading the old target off the terminator first so the phi
        // maintenance and the delete-edge log can name it. The terminator kind
        // is unchanged since the caller's witness (single-threaded, no
        // intervening mutation).
        let old_id = match slot {
            EditSlot::SwitchDefault => {
                let crate::value::ValueKindData::Instruction(i) = &ctx.value_data(term_id).kind
                else {
                    unreachable!("redirect_slot: terminator is not an instruction");
                };
                let crate::instruction::InstructionKindData::Switch(switch) = &i.kind else {
                    unreachable!("redirect_slot: switch slot on a non-switch terminator");
                };
                let old = switch.default_bb.get();
                switch.default_bb.set(new_id);
                old
            }
            EditSlot::SwitchCase(old) => {
                let crate::value::ValueKindData::Instruction(i) = &ctx.value_data(term_id).kind
                else {
                    unreachable!("redirect_slot: terminator is not an instruction");
                };
                let crate::instruction::InstructionKindData::Switch(switch) = &i.kind else {
                    unreachable!("redirect_slot: switch slot on a non-switch terminator");
                };
                for entry in switch.cases.borrow_mut().iter_mut() {
                    if entry.1 == old {
                        entry.1 = new_id;
                    }
                }
                old
            }
            EditSlot::BrThen | EditSlot::BrElse | EditSlot::BrUncond => {
                let crate::value::ValueKindData::Instruction(i) = &ctx.value_data(term_id).kind
                else {
                    unreachable!("redirect_slot: terminator is not an instruction");
                };
                let crate::instruction::InstructionKindData::Br(branch) = &i.kind else {
                    unreachable!("redirect_slot: branch slot on a non-branch terminator");
                };
                let mut kind = branch.kind.borrow_mut();
                match (&mut *kind, slot) {
                    (crate::instr_types::BranchKind::Unconditional(target), EditSlot::BrUncond) => {
                        let old = *target;
                        *target = new_id;
                        old
                    }
                    (
                        crate::instr_types::BranchKind::Conditional { then_bb, .. },
                        EditSlot::BrThen,
                    ) => {
                        let old = *then_bb;
                        *then_bb = new_id;
                        old
                    }
                    (
                        crate::instr_types::BranchKind::Conditional { else_bb, .. },
                        EditSlot::BrElse,
                    ) => {
                        let old = *else_bb;
                        *else_bb = new_id;
                        old
                    }
                    _ => unreachable!("redirect_slot: branch terminator yielded a non-branch slot"),
                }
            }
            EditSlot::InvokeNormal | EditSlot::InvokeUnwind => {
                let crate::value::ValueKindData::Instruction(i) = &ctx.value_data(term_id).kind
                else {
                    unreachable!("redirect_slot: terminator is not an instruction");
                };
                let crate::instruction::InstructionKindData::Invoke(invoke) = &i.kind else {
                    unreachable!("redirect_slot: invoke slot on a non-invoke terminator");
                };
                match slot {
                    EditSlot::InvokeNormal => {
                        let old = invoke.normal_dest.get();
                        invoke.normal_dest.set(new_id);
                        old
                    }
                    EditSlot::InvokeUnwind => {
                        let old = invoke.unwind_dest.get();
                        invoke.unwind_dest.set(new_id);
                        old
                    }
                    _ => unreachable!("redirect_slot: invoke arm restricts slot to an invoke dest"),
                }
            }
            EditSlot::CallBrDefault => {
                let crate::value::ValueKindData::Instruction(i) = &ctx.value_data(term_id).kind
                else {
                    unreachable!("redirect_slot: terminator is not an instruction");
                };
                let crate::instruction::InstructionKindData::CallBr(callbr) = &i.kind else {
                    unreachable!("redirect_slot: callbr slot on a non-callbr terminator");
                };
                let old = callbr.default_dest.get();
                callbr.default_dest.set(new_id);
                old
            }
            EditSlot::CallBrIndirect(idx) => {
                let crate::value::ValueKindData::Instruction(i) = &ctx.value_data(term_id).kind
                else {
                    unreachable!("redirect_slot: terminator is not an instruction");
                };
                let crate::instruction::InstructionKindData::CallBr(callbr) = &i.kind else {
                    unreachable!("redirect_slot: callbr slot on a non-callbr terminator");
                };
                // Bounds-check the caller-supplied index before mutating: an
                // out-of-range index is rejected here, before the dest cell or
                // any phi is touched, so a bad index leaves the IR untouched.
                let cell = callbr.indirect_dests.get(idx).ok_or(IrError::InvalidOperation {
                    message: "redirect_indirect: `callbr` indirect destination index out of range",
                })?;
                let old = cell.get();
                cell.set(new_id);
                old
            }
        };

        // Drop `from`'s incomings from the old target's phis, keeping one per
        // surviving parallel `from → old` edge, then add the typed incomings to
        // `new_to`'s phis (each registered in its value's use-list by the
        // edge-add path). Retargeting one arm of a `cond_br %c, X, X` (or one
        // `switch` case when the default also targets that block) leaves `from`
        // still reaching `old` through the untouched parallel edge; the survivor
        // count is read off the now-mutated terminator (0 for the normal
        // single-edge redirect). All blocks share the module's label type, so
        // `from`'s block carries the same module ref and label type as the old
        // target.
        let old_block =
            BasicBlock::<Dyn, Terminated, B>::from_parts(old_id, from_block.module, from_block.ty);
        let surviving = crate::cfg::block_successors(&from_block)
            .iter()
            .filter(|succ| succ.as_value().id == old_id)
            .count();
        self.drop_incoming_from_pred(&old_block, from_id, surviving)?;

        let builder = IRBuilder::new(self.patch.module_mut());
        for (phi_id, value) in param_phis.iter().zip(phi_values.iter()) {
            let phi_ty = ctx.value_data(*phi_id).ty;
            let phi_val = Value::from_parts(*phi_id, new_to.module, phi_ty);
            // This add cannot fail: arity and per-phi type were validated above,
            // and the ambiguous-duplicate check cannot fire because `from` was
            // proven not to already reach `new_to`. The `?` is defensive — if
            // either precondition ever weakens, an error here would leave a
            // partial redirect, so keep both guards ahead of this loop.
            builder.phi_add_incoming_from_value(phi_val, *value, from_block.copy_handle())?;
        }

        {
            let mut log = self.cfg_updates.borrow_mut();
            log.push(CfgUpdate::delete(from_id, old_id));
            log.push(CfgUpdate::insert(from_id, new_id));
        }
        self.patch.dirty.set(true);
        Ok(())
    }

    /// Read the current `default` destination id of the `switch` terminator
    /// `term_id`. Used by [`SwitchEdit::remove_successor`] to reject removing
    /// the default edge (a `switch` must keep its default).
    fn switch_default_dest(&self, term_id: ValueId) -> ValueId {
        let ctx = self.patch.module_mut().core_ref().context();
        let crate::value::ValueKindData::Instruction(i) = &ctx.value_data(term_id).kind else {
            unreachable!("switch_default_dest: terminator is not an instruction");
        };
        let crate::instruction::InstructionKindData::Switch(switch) = &i.kind else {
            unreachable!("switch_default_dest: `SwitchEdit` term is not a switch");
        };
        switch.default_bb.get()
    }

    /// Whether any case of the `switch` terminator `term_id` targets `dest_id`.
    /// Used by [`SwitchEdit::redirect_successor`]/[`SwitchEdit::remove_successor`]
    /// to reject an `old_to` that is not a live case successor of the switch
    /// (the target-based typed ops carry `old_to` in the slot, so — unlike the
    /// role-named ops — they must witness that it names a real case; the
    /// classic "is not a successor" rejection). The
    /// default edge is deliberately not a case, so this returns `false` for it.
    fn switch_has_case_successor(&self, term_id: ValueId, dest_id: ValueId) -> bool {
        let ctx = self.patch.module_mut().core_ref().context();
        let crate::value::ValueKindData::Instruction(i) = &ctx.value_data(term_id).kind else {
            unreachable!("switch_has_case_successor: terminator is not an instruction");
        };
        let crate::instruction::InstructionKindData::Switch(switch) = &i.kind else {
            unreachable!("switch_has_case_successor: `SwitchEdit` term is not a switch");
        };
        switch.cases.borrow().iter().any(|entry| entry.1 == dest_id)
    }

    /// Narrow `from`'s terminator into a typed edit handle, choosing the arm
    /// by opcode (and splitting `br` from `cond_br` by
    /// [`is_conditional`](crate::BranchInst::is_conditional)). The returned
    /// [`TermEdit`] *type* fixes which edge ops exist — a `br` yields a
    /// [`BrEdit`] with only `redirect`, a `cond_br` a [`CondBrEdit`] with
    /// `remove_then`/`remove_else`, an `invoke`/`callbr` a handle with no
    /// `remove_*` at all — so a structurally-invalid edge edit (e.g. removing
    /// an `invoke`'s normal edge) is a *compile* error rather than a runtime
    /// rejection. Terminators with no editable edges here (`ret`,
    /// `unreachable`, `indirectbr`, and the exception-handling terminators)
    /// come back as [`TermEdit::Uneditable`] carrying the raw view.
    ///
    /// Single-use: a handle (this factory's result and the [`Self::edit_switch`]
    /// / [`Self::edit_cond_br`] / [`Self::edit_br`] / [`Self::edit_invoke`] /
    /// [`Self::edit_callbr`] narrows) caches `from`'s terminator id. Obtain a
    /// handle, perform one edit; do not retain a handle across an intervening op
    /// that replaces `from`'s terminator (e.g. a `remove_*` collapse, or a later
    /// narrow) and then reuse it — the cached id is stale and an edge op on it
    /// would hit an `unreachable!`. Re-narrow with a fresh `edit_*` instead.
    ///
    /// Errors: [`IrError::InvalidOperation`] if `from` has no terminator.
    #[inline]
    pub fn edit_terminator<'e>(
        &'e self,
        from: &BasicBlockView<'ctx, B>,
    ) -> IrResult<TermEdit<'e, 'm, 'r, 'ctx, B, R>> {
        let from_block = from.as_basic_block();
        let term = from_block.terminator().ok_or(IrError::InvalidOperation {
            message: "edit_terminator: `from` has no terminator",
        })?;
        let term_id = term.as_value().id;
        let from_view = from.clone();
        let kind = term.terminator_kind().ok_or(IrError::InvalidOperation {
            message: "edit_terminator: `from`'s last instruction is not a terminator",
        })?;
        Ok(match kind {
            TerminatorKind::Br(branch) => {
                if branch.is_conditional() {
                    TermEdit::CondBr(CondBrEdit {
                        reshape: self,
                        from: from_view,
                        term_id,
                    })
                } else {
                    TermEdit::Br(BrEdit {
                        reshape: self,
                        from: from_view,
                        term_id,
                    })
                }
            }
            TerminatorKind::Switch(_) => TermEdit::Switch(SwitchEdit {
                reshape: self,
                from: from_view,
                term_id,
            }),
            TerminatorKind::Invoke(_) => TermEdit::Invoke(InvokeEdit {
                reshape: self,
                from: from_view,
                term_id,
            }),
            TerminatorKind::CallBr(_) => TermEdit::CallBr(CallBrEdit {
                reshape: self,
                from: from_view,
                term_id,
            }),
            TerminatorKind::Ret(_)
            | TerminatorKind::IndirectBr(_)
            | TerminatorKind::Resume(_)
            | TerminatorKind::CatchReturn(_)
            | TerminatorKind::CleanupReturn(_)
            | TerminatorKind::CatchSwitch(_)
            | TerminatorKind::Unreachable(_) => TermEdit::Uneditable(term),
        })
    }

    /// `dyn_cast`-style narrow to a [`SwitchEdit`]: `Err` if `from`'s
    /// terminator is not a `switch`.
    ///
    /// Errors: [`IrError::InvalidOperation`] if `from` has no terminator or its
    /// terminator is not a `switch`.
    #[inline]
    pub fn edit_switch<'e>(
        &'e self,
        from: &BasicBlockView<'ctx, B>,
    ) -> IrResult<SwitchEdit<'e, 'm, 'r, 'ctx, B, R>> {
        match self.edit_terminator(from)? {
            TermEdit::Switch(edit) => Ok(edit),
            _ => Err(IrError::InvalidOperation {
                message: "edit_switch: `from`'s terminator is not a `switch`",
            }),
        }
    }

    /// `dyn_cast`-style narrow to a [`CondBrEdit`]: `Err` if `from`'s
    /// terminator is not a conditional `br`.
    ///
    /// Errors: [`IrError::InvalidOperation`] if `from` has no terminator or its
    /// terminator is not a `cond_br`.
    #[inline]
    pub fn edit_cond_br<'e>(
        &'e self,
        from: &BasicBlockView<'ctx, B>,
    ) -> IrResult<CondBrEdit<'e, 'm, 'r, 'ctx, B, R>> {
        match self.edit_terminator(from)? {
            TermEdit::CondBr(edit) => Ok(edit),
            _ => Err(IrError::InvalidOperation {
                message: "edit_cond_br: `from`'s terminator is not a `cond_br`",
            }),
        }
    }

    /// `dyn_cast`-style narrow to a [`BrEdit`]: `Err` if `from`'s terminator is
    /// not an *unconditional* `br` (a `cond_br` is rejected — narrow it with
    /// [`Self::edit_cond_br`] instead).
    ///
    /// Errors: [`IrError::InvalidOperation`] if `from` has no terminator or its
    /// terminator is not an unconditional `br`.
    #[inline]
    pub fn edit_br<'e>(
        &'e self,
        from: &BasicBlockView<'ctx, B>,
    ) -> IrResult<BrEdit<'e, 'm, 'r, 'ctx, B, R>> {
        match self.edit_terminator(from)? {
            TermEdit::Br(edit) => Ok(edit),
            _ => Err(IrError::InvalidOperation {
                message: "edit_br: `from`'s terminator is not an unconditional `br`",
            }),
        }
    }

    /// `dyn_cast`-style narrow to an [`InvokeEdit`]: `Err` if `from`'s
    /// terminator is not an `invoke`.
    ///
    /// Errors: [`IrError::InvalidOperation`] if `from` has no terminator or its
    /// terminator is not an `invoke`.
    #[inline]
    pub fn edit_invoke<'e>(
        &'e self,
        from: &BasicBlockView<'ctx, B>,
    ) -> IrResult<InvokeEdit<'e, 'm, 'r, 'ctx, B, R>> {
        match self.edit_terminator(from)? {
            TermEdit::Invoke(edit) => Ok(edit),
            _ => Err(IrError::InvalidOperation {
                message: "edit_invoke: `from`'s terminator is not an `invoke`",
            }),
        }
    }

    /// `dyn_cast`-style narrow to a [`CallBrEdit`]: `Err` if `from`'s
    /// terminator is not a `callbr`.
    ///
    /// Errors: [`IrError::InvalidOperation`] if `from` has no terminator or its
    /// terminator is not a `callbr`.
    #[inline]
    pub fn edit_callbr<'e>(
        &'e self,
        from: &BasicBlockView<'ctx, B>,
    ) -> IrResult<CallBrEdit<'e, 'm, 'r, 'ctx, B, R>> {
        match self.edit_terminator(from)? {
            TermEdit::CallBr(edit) => Ok(edit),
            _ => Err(IrError::InvalidOperation {
                message: "edit_callbr: `from`'s terminator is not a `callbr`",
            }),
        }
    }

    /// Insert a fully-witnessed phi at `block`'s phi head and return its erased
    /// result [`Value`].
    ///
    /// Do not `insert_phi` into a block whose leading phis are block parameters
    /// (created by [`IRBuilder::append_block_with_params`](crate::IRBuilder)):
    /// the new phi lands in that same head group, where the block-argument path
    /// would miscount it as an extra block parameter.
    ///
    /// Because a [`ReshapeCfg`] pass sees a *complete* CFG, every SSA obligation
    /// a phi carries is witnessed here at the call — none is deferred to
    /// [`Module::verify`](crate::Module::verify):
    ///
    /// 1. **Completeness / type / differing-duplicate** — `block`'s predecessor
    ///    multiset is rebuilt by inverting the CFG successors of the function's
    ///    blocks (edge multiplicity preserved), then the authoritative
    ///    per-phi coherence check the verifier and parser share is run over the
    ///    incomings.
    /// 2. **Incoming-value dominance** — each incoming value must dominate the
    ///    edge it flows in on. A value defined in an instruction must have its
    ///    defining block dominate the incoming's predecessor (inclusive: a value
    ///    defined *in* the predecessor dominates the edge); a non-instruction
    ///    operand (parameter / constant / global) dominates every block. The
    ///    verdicts are read from the **repaired** dominator tree via
    ///    [`Self::analysis_repaired`].
    ///
    /// The borrow of the dominator tree is `&mut self`-tied, so it is read and
    /// every dominance verdict is settled *before* any IR mutation; the phi is
    /// created only once all obligations are witnessed. A phi insertion adds no
    /// CFG edge, so no [`CfgUpdate`] is recorded — but the module changed, so the
    /// mutator is marked dirty.
    ///
    /// The pass must declare [`DominatorTreeAnalysis`] in its `Requires`: the
    /// `R: AnalysisSelector<.., DominatorTreeAnalysis, _>` bound below is
    /// unsatisfiable otherwise, so a pass that forgot it fails to compile — a
    /// type-level nudge rather than a runtime surprise.
    ///
    /// Errors: [`IrError::PhiIncomingNotDominating`] if some incoming value does
    /// not dominate its edge; a coherence [`IrError`] (mapped from the shared
    /// phi check) if the incomings are incomplete, mistyped, or carry a
    /// differing duplicate for one predecessor.
    #[inline]
    pub fn insert_phi<I>(
        &mut self,
        block: &BasicBlockView<'ctx, B>,
        ty: Type<'ctx, B>,
        incomings: &[(Value<'ctx, B>, BasicBlockLabel<'ctx, Dyn, B>)],
    ) -> IrResult<Value<'ctx, B>>
    where
        R: AnalysisSelector<'ctx, B, DominatorTreeAnalysis, I>,
    {
        let target_id = block.as_basic_block().as_value().id;

        // (1) Predecessor multiset of `block`: invert `block_successors` over
        // the function's blocks (the pattern `check_function_phi_coherence`
        // uses), keeping edge multiplicity so a multi-edge is counted once per
        // edge.
        let mut preds: Vec<ValueId> = Vec::new();
        for bb in self.function().basic_blocks() {
            let handle = bb.as_basic_block();
            let pred_id = handle.as_value().id;
            for succ in crate::cfg::block_successors(&handle) {
                if succ.as_value().id == target_id {
                    preds.push(pred_id);
                }
            }
        }

        // (2) Completeness / type / differing-duplicate: the authoritative
        // per-phi coherence algorithm, mapped to an `IrError` on failure.
        let ty_id = ty.id();
        let incoming_ids: Vec<(ValueId, ValueId)> = incomings
            .iter()
            .map(|(value, pred)| (value.id, pred.as_value().id))
            .collect();
        let ctx = self.patch.module_mut().core_ref().context();
        let value_ty_of = |id: ValueId| ctx.value_data(id).ty;
        if let Err(violation) = check_phi_incoming(ty_id, &incoming_ids, &preds, &value_ty_of) {
            // Same renderer the .ll parser uses for its coherence diagnostics,
            // so a pass-side insert and a parse report the same message for the
            // same failure.
            return Err(IrError::PhiCoherence {
                message: render_phi_violation(&violation, ty_id, self.patch.module_mut()),
            });
        }

        // (3) SSA dominance. `dt` borrows `&mut self`, so settle every verdict
        // into owned ids and drop the borrow BEFORE any IR mutation runs.
        let mut dom_failure: Option<(ValueId, ValueId)> = None;
        {
            let dt = self.analysis_repaired::<DominatorTreeAnalysis, _>();
            for (value, pred) in incomings {
                // Only instruction operands carry a dominance obligation; a
                // parameter / constant / global dominates every block.
                if let Ok(inst) = InstructionView::try_from(*value) {
                    let def_block = inst.parent();
                    if !dt.dominates_block(def_block, *pred) {
                        dom_failure = Some((def_block.as_value().id, pred.as_value().id));
                        break;
                    }
                }
            }
        }
        if let Some((value_block, pred_block)) = dom_failure {
            let ctx = self.patch.module_mut().core_ref().context();
            return Err(IrError::PhiIncomingNotDominating {
                value_block: ctx.block_diag_name(value_block),
                pred_block: ctx.block_diag_name(pred_block),
            });
        }

        // (4) All witnessed — create the phi at `block`'s phi head and record
        // every incoming. `make_phi_in_block` is cursor-independent, so an
        // Unpositioned builder is fine.
        let builder = IRBuilder::new(self.patch.module_mut());
        let phi_val = builder.make_phi_in_block(target_id, ty_id, "");
        for (value, pred) in incomings {
            let pred_block =
                BasicBlock::<Dyn, Terminated, B>::from_parts(pred.id, pred.module, pred.ty);
            builder.phi_add_incoming_from_value(phi_val, *value, pred_block)?;
        }

        // (5) No CFG edge changed, so no `CfgUpdate` is recorded — but the
        // module did change.
        self.patch.dirty.set(true);
        Ok(phi_val)
    }

    /// Finish: report the [`ReshapeCfg`] preservation floor (`none()` — nothing
    /// preserved) if anything was mutated, or everything-preserved if the run
    /// was a no-op. Consumes the mutator; the no-op case is witnessed by the
    /// dirty flag.
    ///
    /// The recorded [`CfgUpdate`] log rides out with the report so the driver's
    /// `done()`-flush can offer it to cached CFG analyses — a floor of `none()`
    /// is the *starting* point, and the framework then adds back exactly the
    /// analyses it witnesses repair (never an author claim).
    #[inline]
    pub fn done(self) -> FnReport {
        let dirty = self.patch.is_dirty();
        let updates = self.cfg_updates.into_inner();
        let pa = if dirty {
            <ReshapeCfg as FnAccess>::preserved_floor()
        } else {
            PreservedAnalyses::all()
        };
        FnReport::from_pa_with_cfg_updates(pa, updates)
    }
}

// NB: `FnReshape` deliberately does *not* `Deref` to `FnPatch`. A blanket
// `Deref` would re-expose `FnPatch::analysis`, whose `&'r`-borrowed reference
// outlives a reshape edit and so would reintroduce the mid-reshape stale-read
// footgun this rung exists to eliminate. The in-block-edit surface is delegated
// by hand above; CFG analyses are read only through the `&mut self`-tied
// [`FnReshape::analysis_repaired`].

// ==========================================================================
// Typed terminator edit handles
// ==========================================================================
//
// Each handle narrows one terminator into a value whose *type* fixes which edge
// ops exist, so a structurally-invalid CFG edge edit is a *compile* error, not a
// runtime rejection:
//
//   * an unconditional `br` has exactly one edge that cannot be removed (nothing
//     would remain), so [`BrEdit`] has `redirect` but no `remove`;
//   * an `invoke`/`callbr` edge is never removable, so [`InvokeEdit`]/
//     [`CallBrEdit`] carry only `redirect_*`;
//   * a `switch` must keep its default, so [`SwitchEdit`] has `remove_successor`
//     but no `remove_default`;
//   * collapsing a `cond_br` consumes one arm, so [`CondBrEdit::remove_then`]/
//     `remove_else` take `self` by value (a second collapse is a use-after-move).
//
// The `remove_*` methods live on NO shared trait — they are inherent methods on
// the one handle each belongs to — so `invoke_edit.remove_normal(..)` is an
// `E0599 no method`, proven by the compile-fail fixtures. Every method delegates
// to the shared, single-validated [`FnReshape::redirect_slot`]/
// [`FnReshape::remove_slot`], so the typed surface inherits the same phi and
// edge validation as the legacy edge ops for free.

/// Typed edit handle for an unconditional `br` terminator. Its sole edge cannot
/// be removed (the block would be left with no successor), so this handle offers
/// only [`redirect`](Self::redirect).
pub struct BrEdit<'e, 'm, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    reshape: &'e FnReshape<'m, 'r, 'ctx, B, R>,
    from: BasicBlockView<'ctx, B>,
    term_id: ValueId,
}

impl<'e, 'm, 'r, 'ctx, B, R> BrEdit<'e, 'm, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    /// Retarget the branch's sole edge onto `new_to`, seeding `new_to`'s leading
    /// phis with `phi_values`. Runs through the shared, single-validated redirect
    /// path; see [`SwitchEdit::redirect_successor`] for the phi-values contract
    /// and errors.
    #[inline]
    pub fn redirect(
        &self,
        new_to: &BasicBlockLabel<'ctx, Dyn, B>,
        phi_values: &[Value<'ctx, B>],
    ) -> IrResult<()> {
        self.reshape.redirect_slot(
            &self.from,
            self.term_id,
            EditSlot::BrUncond,
            new_to,
            phi_values,
        )
    }
}

/// Typed edit handle for a conditional `br` (`cond_br`). Either arm may be
/// redirected; removing an arm collapses the `cond_br` to an unconditional `br`
/// to the survivor, so the `remove_*` methods consume `self` — a `cond_br` has
/// exactly one collapse, and a second would be a use-after-move.
pub struct CondBrEdit<'e, 'm, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    reshape: &'e FnReshape<'m, 'r, 'ctx, B, R>,
    from: BasicBlockView<'ctx, B>,
    term_id: ValueId,
}

impl<'e, 'm, 'r, 'ctx, B, R> CondBrEdit<'e, 'm, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    /// Retarget the `then` arm onto `new_to`, seeding `new_to`'s leading phis
    /// with `phi_values`. See [`SwitchEdit::redirect_successor`] for the
    /// phi-values contract and errors.
    #[inline]
    pub fn redirect_then(
        &self,
        new_to: &BasicBlockLabel<'ctx, Dyn, B>,
        phi_values: &[Value<'ctx, B>],
    ) -> IrResult<()> {
        self.reshape.redirect_slot(
            &self.from,
            self.term_id,
            EditSlot::BrThen,
            new_to,
            phi_values,
        )
    }

    /// Retarget the `else` arm onto `new_to`, seeding `new_to`'s leading phis
    /// with `phi_values`. See [`SwitchEdit::redirect_successor`] for the
    /// phi-values contract and errors.
    #[inline]
    pub fn redirect_else(
        &self,
        new_to: &BasicBlockLabel<'ctx, Dyn, B>,
        phi_values: &[Value<'ctx, B>],
    ) -> IrResult<()> {
        self.reshape.redirect_slot(
            &self.from,
            self.term_id,
            EditSlot::BrElse,
            new_to,
            phi_values,
        )
    }

    /// Remove the `then` edge, collapsing the `cond_br` to an unconditional `br`
    /// to the surviving `else` target and deregistering the now-dead condition
    /// operand. Consumes `self`: the `cond_br` no longer exists after the
    /// collapse, so a second removal cannot be spelled. See
    /// [`SwitchEdit::remove_successor`] for the shared phi maintenance.
    #[inline]
    pub fn remove_then(self) -> IrResult<()> {
        self.reshape
            .remove_slot(&self.from, self.term_id, EditSlot::BrThen)
    }

    /// Remove the `else` edge, collapsing the `cond_br` to an unconditional `br`
    /// to the surviving `then` target. Consumes `self` (see
    /// [`Self::remove_then`]).
    #[inline]
    pub fn remove_else(self) -> IrResult<()> {
        self.reshape
            .remove_slot(&self.from, self.term_id, EditSlot::BrElse)
    }
}

/// Typed edit handle for a `switch`. A case edge (identified by its old target)
/// or the default edge may be redirected; a case edge may be removed, but the
/// default may not — a `switch` must keep its default — so there is no
/// `remove_default`.
pub struct SwitchEdit<'e, 'm, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    reshape: &'e FnReshape<'m, 'r, 'ctx, B, R>,
    from: BasicBlockView<'ctx, B>,
    term_id: ValueId,
}

impl<'e, 'm, 'r, 'ctx, B, R> SwitchEdit<'e, 'm, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    /// Retarget every case whose destination is `old_to` onto `new_to`,
    /// dropping `old_to`'s `from`-incomings and seeding `new_to`'s leading phis
    /// with `phi_values` — all as one op. This is the **canonical redirect
    /// contract**; every other `redirect_*` on the typed edit handles shares it.
    ///
    /// **Typed phi values.** `phi_values` is aligned to `new_to`'s leading phis
    /// (its block parameters), one value per phi: its length must equal that
    /// count ([`IrError::PhiArgArityMismatch`]) and each value's type must match
    /// its phi ([`IrError::TypeMismatch`]). Both are checked, together with the
    /// edge precondition, *before* any mutation, so a rejected call leaves the
    /// terminator and every phi untouched. Dominance of each supplied value over
    /// the new edge is the caller's obligation, settled at
    /// [`Module::verify`](crate::Module::verify).
    ///
    /// **Phi maintenance.** `old_to`'s leading phis lose their `from` incomings;
    /// if `from` was `old_to`'s only predecessor, a phi thereby emptied is
    /// replaced with poison of its own type and erased (LLVM `removePredecessor`
    /// parity, keeping the result round-trippable). `new_to`'s leading phis each
    /// gain a `(phi_values[i], from)` incoming, registered in the value's
    /// use-list so RAUW stays correct.
    ///
    /// Records a [`CfgUpdate`] delete + insert pair and marks the mutator dirty.
    ///
    /// Errors: [`IrError::InvalidOperation`] if `old_to` is not a case successor
    /// of this switch or `from` already reaches `new_to`;
    /// [`IrError::PhiArgArityMismatch`] / [`IrError::TypeMismatch`] for a
    /// mis-sized or mistyped `phi_values`.
    #[inline]
    pub fn redirect_successor(
        &self,
        old_to: &BasicBlockLabel<'ctx, Dyn, B>,
        new_to: &BasicBlockLabel<'ctx, Dyn, B>,
        phi_values: &[Value<'ctx, B>],
    ) -> IrResult<()> {
        let old_id = old_to.as_value().id;
        // `old_to` is target-based, so witness it names a live case before
        // delegating: a bogus `old_to` retargets zero cases yet the shared tail
        // would still seed `new_to`'s phis / log a spurious `CfgUpdate`. This
        // also rejects `old_to == default` (the default is not a case; edit it
        // with [`Self::redirect_default`]).
        if !self.reshape.switch_has_case_successor(self.term_id, old_id) {
            return Err(IrError::InvalidOperation {
                message: "redirect_successor: `old_to` is not a case successor of this switch",
            });
        }
        self.reshape.redirect_slot(
            &self.from,
            self.term_id,
            EditSlot::SwitchCase(old_id),
            new_to,
            phi_values,
        )
    }

    /// Retarget the default edge onto `new_to`, seeding `new_to`'s leading phis
    /// with `phi_values`. See [`Self::redirect_successor`] for the phi-values
    /// contract and errors.
    #[inline]
    pub fn redirect_default(
        &self,
        new_to: &BasicBlockLabel<'ctx, Dyn, B>,
        phi_values: &[Value<'ctx, B>],
    ) -> IrResult<()> {
        self.reshape.redirect_slot(
            &self.from,
            self.term_id,
            EditSlot::SwitchDefault,
            new_to,
            phi_values,
        )
    }

    /// Remove every case whose destination is `old_to`, collapsing the
    /// `from → old_to` edge (the leftover `switch` keeps its default). This is
    /// the **canonical edge-removal contract** for its phi maintenance, shared
    /// by [`CondBrEdit::remove_then`]/[`CondBrEdit::remove_else`].
    ///
    /// **Phi maintenance.** Once the edge is gone `from` is no longer a
    /// predecessor of `old_to`, so every leading phi in `old_to` loses all its
    /// `(value, from)` incomings (LLVM `removePredecessor`), each removed value's
    /// use-list updated so RAUW stays correct. A phi left with a single entry is
    /// legal (the uniform-phi fold cleans it later); if `from` was `old_to`'s
    /// *only* predecessor, a phi left with **zero** incomings — which has no
    /// legal textual form, so the module would not round-trip — is replaced with
    /// poison of its own type and erased.
    ///
    /// Records a [`CfgUpdate`] delete and marks the mutator dirty.
    ///
    /// Errors: [`IrError::InvalidOperation`] if `old_to` is the switch's default
    /// edge (a `switch` must keep its default) or is not a case successor.
    #[inline]
    pub fn remove_successor(&self, old_to: &BasicBlockLabel<'ctx, Dyn, B>) -> IrResult<()> {
        let old_id = old_to.as_value().id;
        if self.reshape.switch_default_dest(self.term_id) == old_id {
            return Err(IrError::InvalidOperation {
                message: "remove_successor: cannot remove a `switch`'s default edge",
            });
        }
        // `old_to` is target-based: witness it names a live case before
        // delegating, else `remove_slot` drops zero cases yet still logs a
        // spurious `CfgUpdate::delete` that corrupts incremental CFG analysis.
        if !self.reshape.switch_has_case_successor(self.term_id, old_id) {
            return Err(IrError::InvalidOperation {
                message: "remove_successor: `old_to` is not a case successor of this switch",
            });
        }
        self.reshape
            .remove_slot(&self.from, self.term_id, EditSlot::SwitchCase(old_id))
    }
}

/// Typed edit handle for an `invoke`. Both destinations (normal and unwind) may
/// be redirected; neither may be removed — an `invoke` always has exactly two
/// edges — so this handle carries only `redirect_*`.
pub struct InvokeEdit<'e, 'm, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    reshape: &'e FnReshape<'m, 'r, 'ctx, B, R>,
    from: BasicBlockView<'ctx, B>,
    term_id: ValueId,
}

impl<'e, 'm, 'r, 'ctx, B, R> InvokeEdit<'e, 'm, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    /// Retarget the normal (non-unwind) edge onto `new_to`, seeding `new_to`'s
    /// leading phis with `phi_values`. See [`SwitchEdit::redirect_successor`] for
    /// the phi-values contract and errors.
    #[inline]
    pub fn redirect_normal(
        &self,
        new_to: &BasicBlockLabel<'ctx, Dyn, B>,
        phi_values: &[Value<'ctx, B>],
    ) -> IrResult<()> {
        self.reshape.redirect_slot(
            &self.from,
            self.term_id,
            EditSlot::InvokeNormal,
            new_to,
            phi_values,
        )
    }

    /// Retarget the unwind edge onto `new_to`, seeding `new_to`'s leading phis
    /// with `phi_values`. See [`SwitchEdit::redirect_successor`] for the
    /// phi-values contract and errors.
    #[inline]
    pub fn redirect_unwind(
        &self,
        new_to: &BasicBlockLabel<'ctx, Dyn, B>,
        phi_values: &[Value<'ctx, B>],
    ) -> IrResult<()> {
        self.reshape.redirect_slot(
            &self.from,
            self.term_id,
            EditSlot::InvokeUnwind,
            new_to,
            phi_values,
        )
    }
}

/// Typed edit handle for a `callbr`. The default (fallthrough) edge and each
/// indirect edge may be redirected; none may be removed, so this handle carries
/// only `redirect_*`.
pub struct CallBrEdit<'e, 'm, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    reshape: &'e FnReshape<'m, 'r, 'ctx, B, R>,
    from: BasicBlockView<'ctx, B>,
    term_id: ValueId,
}

impl<'e, 'm, 'r, 'ctx, B, R> CallBrEdit<'e, 'm, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    /// Retarget the default (fallthrough) edge onto `new_to`, seeding `new_to`'s
    /// leading phis with `phi_values`. See [`SwitchEdit::redirect_successor`] for
    /// the phi-values contract and errors.
    #[inline]
    pub fn redirect_default(
        &self,
        new_to: &BasicBlockLabel<'ctx, Dyn, B>,
        phi_values: &[Value<'ctx, B>],
    ) -> IrResult<()> {
        self.reshape.redirect_slot(
            &self.from,
            self.term_id,
            EditSlot::CallBrDefault,
            new_to,
            phi_values,
        )
    }

    /// Retarget the `index`-th indirect edge onto `new_to`, seeding `new_to`'s
    /// leading phis with `phi_values`. An out-of-range `index` is rejected with
    /// [`IrError::InvalidOperation`]. See [`SwitchEdit::redirect_successor`] for
    /// the phi-values contract.
    #[inline]
    pub fn redirect_indirect(
        &self,
        index: usize,
        new_to: &BasicBlockLabel<'ctx, Dyn, B>,
        phi_values: &[Value<'ctx, B>],
    ) -> IrResult<()> {
        self.reshape.redirect_slot(
            &self.from,
            self.term_id,
            EditSlot::CallBrIndirect(index),
            new_to,
            phi_values,
        )
    }
}

/// A terminator narrowed by [`FnReshape::edit_terminator`] into whichever typed
/// edit handle its opcode admits. Matching on this enum recovers the concrete
/// handle whose method set encodes the legal edge ops (see the module-level
/// note on the typed edit handles). Terminators with no editable edges here
/// (`ret`, `unreachable`, `indirectbr`, and the exception-handling terminators)
/// come back as [`Self::Uneditable`].
pub enum TermEdit<'e, 'm, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    /// An unconditional `br`.
    Br(BrEdit<'e, 'm, 'r, 'ctx, B, R>),
    /// A conditional `br` (`cond_br`).
    CondBr(CondBrEdit<'e, 'm, 'r, 'ctx, B, R>),
    /// A `switch`.
    Switch(SwitchEdit<'e, 'm, 'r, 'ctx, B, R>),
    /// An `invoke`.
    Invoke(InvokeEdit<'e, 'm, 'r, 'ctx, B, R>),
    /// A `callbr`.
    CallBr(CallBrEdit<'e, 'm, 'r, 'ctx, B, R>),
    /// A terminator this surface does not edge-edit (`ret`, `unreachable`,
    /// `indirectbr`, `resume`, `catchret`, `cleanupret`, `catchswitch`),
    /// carrying its raw read-only view.
    Uneditable(InstructionView<'ctx, B>),
}

impl MutatingFn for PatchBody {
    type Mutator<'m, 'r, 'ctx, B, R>
        = FnPatch<'m, 'r, 'ctx, B, R>
    where
        'ctx: 'm,
        'ctx: 'r,
        B: ModuleBrand + 'ctx,
        R: FunctionAnalysisList<'ctx, B>;

    #[inline]
    fn into_mutator<'m, 'r, 'ctx, B, R>(
        token: Self::Token<'m, 'ctx, B>,
        function: FunctionView<'ctx, B>,
        results: R::ResultRefs<'r>,
    ) -> Self::Mutator<'m, 'r, 'ctx, B, R>
    where
        B: ModuleBrand + 'ctx,
        R: FunctionAnalysisList<'ctx, B>,
        'ctx: 'm,
        'ctx: 'r,
    {
        FnPatch::new(token, function, results)
    }

    #[inline]
    fn mutator_over_module<'m, 'r, 'ctx, B, R>(
        module: &'m Module<'ctx, B, Unverified>,
        function: FunctionView<'ctx, B>,
        results: R::ResultRefs<'r>,
    ) -> Self::Mutator<'m, 'r, 'ctx, B, R>
    where
        B: ModuleBrand + 'ctx,
        R: FunctionAnalysisList<'ctx, B>,
        'ctx: 'm,
        'ctx: 'r,
    {
        FnPatch::new(module, function, results)
    }
}

impl MutatingFn for ReshapeCfg {
    type Mutator<'m, 'r, 'ctx, B, R>
        = FnReshape<'m, 'r, 'ctx, B, R>
    where
        'ctx: 'm,
        'ctx: 'r,
        B: ModuleBrand + 'ctx,
        R: FunctionAnalysisList<'ctx, B>;

    #[inline]
    fn into_mutator<'m, 'r, 'ctx, B, R>(
        token: Self::Token<'m, 'ctx, B>,
        function: FunctionView<'ctx, B>,
        results: R::ResultRefs<'r>,
    ) -> Self::Mutator<'m, 'r, 'ctx, B, R>
    where
        B: ModuleBrand + 'ctx,
        R: FunctionAnalysisList<'ctx, B>,
        'ctx: 'm,
        'ctx: 'r,
    {
        FnReshape::new(token, function, results)
    }

    #[inline]
    fn mutator_over_module<'m, 'r, 'ctx, B, R>(
        module: &'m Module<'ctx, B, Unverified>,
        function: FunctionView<'ctx, B>,
        results: R::ResultRefs<'r>,
    ) -> Self::Mutator<'m, 'r, 'ctx, B, R>
    where
        B: ModuleBrand + 'ctx,
        R: FunctionAnalysisList<'ctx, B>,
        'ctx: 'm,
        'ctx: 'r,
    {
        FnReshape::new(module, function, results)
    }
}

// ==========================================================================
// capability-graded pass API — module report, entry context, and mutator
// ==========================================================================

/// The value a module pass returns. The module-level mirror of [`FnReport`]:
/// wraps the driver-derived [`PreservedAnalyses`]. Its sole fabrication vector,
/// `Self::from_pa`, is `pub(crate)`, so an external author can *name* the type
/// (it appears in a module pass's `run` return) but can only *obtain* one from a
/// [`ModCx`] or a [`ModRewrite`], never mint one that over-claims preservation.
/// That is what makes "rewrote the module but declared everything preserved"
/// unspellable — the report always carries the floor the consumed capability
/// rung structurally allows (D1; D8).
///
/// A distinct type from [`FnReport`], so a module pass cannot return a function
/// report by mistake and vice versa.
pub struct ModReport {
    pa: PreservedAnalyses,
}

impl ModReport {
    /// Wrap a driver-derived preservation set. `pub(crate)` — this is the sole
    /// construction path, so an external author can never fabricate a report that
    /// over-claims preservation. THE honesty guarantee (mirrors
    /// [`FnReport::from_pa`]).
    #[inline]
    pub(crate) fn from_pa(pa: PreservedAnalyses) -> Self {
        Self { pa }
    }

    /// Consume the report and yield its preservation set. The driver-facing seam
    /// ([`crate::pass_manager::run_module_pass`] reads this to drive
    /// invalidation). `pub(crate)`: the single-pass driver is its sole caller,
    /// and reading the set out of a report cannot mint a dishonest one — unlike
    /// [`Self::from_pa`].
    #[inline]
    pub(crate) fn into_pa(self) -> PreservedAnalyses {
        self.pa
    }
}

/// Consuming entry context handed to a module pass at capability rung `A` — the
/// module-level mirror of [`FnCx`], with a four-lifetime shape plus the
/// [`FnCx`] report/mutate flow.
///
/// Parameterized by the access marker `A` (which rung) and the module `Requires`
/// list `R` rather than by a pass trait, so the context type stands alone. The
/// `ModulePass` trait spells its `run` signature as
/// `ModCx<'_, '_, '_, 'ctx, B, Self::Access, Self::Requires>`.
///
/// The typestate that makes a preservation lie unspellable: to change the module
/// a pass must call [`ModCx::mutate`], which **consumes** the context and returns
/// a [`ModRewrite`]. Before `mutate()`, [`ModCx::unchanged`] yields an
/// all-preserved report; after it, the context is gone, so the only report left
/// is the mutator's `done()` → the [`RewriteModule`] floor (`none()` — a module
/// rewrite is the heaviest rung and preserves nothing by default). `Inspect`
/// module passes have no `mutate()` at all.
///
/// The module `token` (`'pm`) borrows the
/// long-lived pipeline module, the prefetched `results` (`'r`) borrow the module
/// analysis manager only for the pass's scope, `mam` (`'r`) is a shared
/// cache-peek borrow, and `fam` (`'f`) is a reborrowed `&mut` for the fallible
/// per-function queries. (llvmkit-specific capability-context lock — no upstream
/// analog: LLVM module-pass contexts are untyped `Module&` + `MAM&`.)
pub struct ModCx<'pm, 'r, 'f, 'ctx, B, A, R>
where
    B: ModuleBrand + 'ctx,
    A: ModAccess,
    R: ModuleAnalysisList<'ctx, B>,
    'ctx: 'pm,
    'ctx: 'r,
    'ctx: 'f,
{
    module: ModuleView<'ctx, B>,
    token: A::Token<'pm, 'ctx, B>,
    results: R::ResultRefs<'r>,
    mam: &'r ModuleAnalysisManager<'ctx, B>,
    fam: &'f mut FunctionAnalysisManager<'ctx, B>,
}

impl<'pm, 'r, 'f, 'ctx, B, A, R> ModCx<'pm, 'r, 'f, 'ctx, B, A, R>
where
    B: ModuleBrand + 'ctx,
    A: ModAccess,
    R: ModuleAnalysisList<'ctx, B>,
    'ctx: 'pm,
    'ctx: 'r,
    'ctx: 'f,
{
    /// Assemble a context from the driver-prefetched parts. The driver-facing
    /// seam: [`crate::pass_manager::run_module_pass`] (in-crate) and these tests
    /// construct contexts here. `pub(crate)` — the honesty guarantee rests on
    /// [`ModReport::from_pa`] being non-public, not on this constructor, and the
    /// single-pass driver is now its sole non-test caller.
    #[inline]
    pub(crate) fn new(
        module: ModuleView<'ctx, B>,
        token: A::Token<'pm, 'ctx, B>,
        results: R::ResultRefs<'r>,
        mam: &'r ModuleAnalysisManager<'ctx, B>,
        fam: &'f mut FunctionAnalysisManager<'ctx, B>,
    ) -> Self {
        Self {
            module,
            token,
            results,
            mam,
            fam,
        }
    }

    /// Read-only module view.
    #[inline]
    pub fn module(&self) -> ModuleView<'ctx, B> {
        self.module
    }

    /// Function views in declaration order.
    #[inline]
    pub fn functions(&self) -> ModuleFunctionViews<'ctx, B> {
        ModuleFunctionViews::new(self.module)
    }

    /// Infallible access to a `Requires`-declared module analysis result. The
    /// position index `I` is inferred; an undeclared analysis has no
    /// [`ModuleAnalysisSelector`] impl and fails to compile.
    #[inline]
    pub fn analysis<A2, I>(&self) -> &'r A2::Result
    where
        A2: ModuleAnalysis<'ctx, B>,
        R: ModuleAnalysisSelector<'ctx, B, A2, I>,
    {
        R::select(&self.results)
    }

    /// Query a function analysis for a function in this module. Deliberately
    /// dynamic (fallible): unlike module-level `Requires`, there is no static
    /// list of which functions a module pass will visit, so per-function analysis
    /// access cannot be prefetched into an infallible accessor.
    #[inline]
    pub fn function_analysis<A2>(
        &mut self,
        function: FunctionView<'ctx, B>,
    ) -> IrResult<&A2::Result>
    where
        A2: FunctionAnalysis<'ctx, B>,
    {
        self.fam.get_result::<A2, _>(function)
    }

    /// Read a cached module analysis without computing it.
    #[inline]
    pub fn cached_module_analysis<A2>(&self) -> Option<&A2::Result>
    where
        A2: ModuleAnalysis<'ctx, B>,
    {
        self.mam.get_cached_result::<A2, _>(self.module)
    }

    /// Finish without mutating: report everything preserved. Available at every
    /// rung ("I inspected / changed nothing"). Consumes the context.
    #[inline]
    pub fn done(self) -> ModReport {
        ModReport::from_pa(PreservedAnalyses::all())
    }
}

impl<'pm, 'r, 'f, 'ctx, B, A, R> ModCx<'pm, 'r, 'f, 'ctx, B, A, R>
where
    B: ModuleBrand + 'ctx,
    A: MutatingModule,
    R: ModuleAnalysisList<'ctx, B>,
    'ctx: 'pm,
    'ctx: 'r,
    'ctx: 'f,
{
    /// The didn't-actually-mutate shortcut: report everything preserved without
    /// entering the mutator. Consumes the context, so it cannot be paired with a
    /// later `mutate()`.
    #[inline]
    pub fn unchanged(self) -> ModReport {
        ModReport::from_pa(PreservedAnalyses::all())
    }

    /// Transition into mutation: **consumes** the context and moves its module
    /// token and prefetched results into the rung's mutator. The `mam`/`fam`
    /// cache-peek borrows end here — the read-only query phase is over (today's
    /// `for_each_function` needs no per-function prefetch). Once called,
    /// `unchanged()`/`done()` on the context are unspellable — the only report
    /// left is the mutator's `done()`, which carries the rung's preservation
    /// floor. This is the core honesty mechanism.
    #[inline]
    pub fn mutate(self) -> <A as MutatingModule>::Mutator<'pm, 'r, 'ctx, B, R> {
        A::into_mutator(self.token, self.results)
    }
}

/// Module-level mutator for the [`RewriteModule`] rung — the heaviest rung, so
/// its `done()` floor is `none()` (nothing preserved). Mutation flows through the
/// shared `&Module<Unverified>` token via interior mutability (never
/// `&mut Module`), the same discipline [`FnPatch`] uses.
///
/// The token exposes the module's existing [`Module::add_global`] /
/// [`Module::add_function`] directly through [`Self::module_mut`]; a sanitizer
/// pass reaches the global/function/constructor "triple" through it. Author sugar
/// for that pattern — `declare_runtime_fn`/`append_ctor`/`add_global` helpers and
/// the `llvm.global_ctors` machinery — is deliberately future work: no in-tree
/// consumer needs it on this branch, and building it now would be speculative.
///
/// Carries the prefetched module-analysis results, so a transform can read module
/// analyses *while* it rewrites (results borrow the analysis manager; mutation
/// borrows the module token — distinct objects, no aliasing).
pub struct ModRewrite<'m, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: ModuleAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    token: &'m Module<'ctx, B, Unverified>,
    results: R::ResultRefs<'r>,
}

impl<'m, 'r, 'ctx, B, R> ModRewrite<'m, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: ModuleAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    #[inline]
    pub(crate) fn new(token: &'m Module<'ctx, B, Unverified>, results: R::ResultRefs<'r>) -> Self {
        Self { token, results }
    }

    /// Read-only module view.
    #[inline]
    pub fn module(&self) -> ModuleView<'ctx, B> {
        self.token.as_view()
    }

    /// Mutation-capable module token, exposing the module's existing
    /// [`Module::add_global`] / [`Module::add_function`] directly.
    #[inline]
    pub fn module_mut(&self) -> &'m Module<'ctx, B, Unverified> {
        self.token
    }

    /// Infallible access to a `Requires`-declared module analysis result *during*
    /// mutation. The results borrow the analysis manager; mutation goes through
    /// the module token — different objects, no aliasing. Mirrors
    /// [`FnPatch::analysis`].
    #[inline]
    pub fn analysis<A2, I>(&self) -> &'r A2::Result
    where
        A2: ModuleAnalysis<'ctx, B>,
        R: ModuleAnalysisSelector<'ctx, B, A2, I>,
    {
        R::select(&self.results)
    }

    /// Visit every function *definition* in module order, handing the visitor a
    /// per-function mutator (`FnPatch`/`FnReshape`, selected by `FnA`) built from
    /// this module's mutation token. Declarations (no entry block) are skipped.
    /// This is the load-bearing module→function visitor; the pass driver calls
    /// `rewrite.for_each_function::<Self::FnAccess>(...)`.
    ///
    /// Per-function analysis prefetch is deliberately future work: each
    /// per-function mutator is built with empty results `()`, so a
    /// `FnPatch::analysis` call inside the visitor has no members to select. A
    /// future revision threads a per-function `Requires` list through here.
    ///
    /// The visitor's mutator is spelled at this mutator's own `'m`/`'r` rather
    /// than fresh higher-ranked lifetimes: the `MutatingFn::Mutator` GAT carries
    /// `'ctx: 'm`/`'ctx: 'r` outlives bounds that a `for<'a, 'b>` quantification
    /// cannot satisfy universally, so a concrete binding is the standalone-green
    /// shape (each mutator is still built fresh per function from the same
    /// module token).
    #[inline]
    pub fn for_each_function<FnA>(
        &mut self,
        mut visitor: impl FnMut(FnA::Mutator<'m, 'r, 'ctx, B, ()>) -> IrResult<()>,
    ) -> IrResult<()>
    where
        FnA: MutatingFn,
    {
        for function in self.module().iter_functions() {
            if function.entry_block().is_none() {
                continue;
            }
            let mutator: FnA::Mutator<'m, 'r, 'ctx, B, ()> =
                FnA::mutator_over_module(self.token, function, ());
            visitor(mutator)?;
        }
        Ok(())
    }

    /// Finish: report the [`RewriteModule`] preservation floor (`none()` —
    /// nothing preserved). Consumes the mutator.
    #[inline]
    pub fn done(self) -> ModReport {
        ModReport::from_pa(<RewriteModule as ModAccess>::preserved_floor())
    }
}

impl MutatingModule for RewriteModule {
    type Mutator<'m, 'r, 'ctx, B, R>
        = ModRewrite<'m, 'r, 'ctx, B, R>
    where
        'ctx: 'm,
        'ctx: 'r,
        B: ModuleBrand + 'ctx,
        R: ModuleAnalysisList<'ctx, B>;

    #[inline]
    fn into_mutator<'m, 'r, 'ctx, B, R>(
        token: Self::Token<'m, 'ctx, B>,
        results: R::ResultRefs<'r>,
    ) -> Self::Mutator<'m, 'r, 'ctx, B, R>
    where
        B: ModuleBrand + 'ctx,
        R: ModuleAnalysisList<'ctx, B>,
        'ctx: 'm,
        'ctx: 'r,
    {
        ModRewrite::new(token, results)
    }
}

#[cfg(test)]
mod tests {
    use super::{FnCx, FunctionView, ModCx};
    use crate::analysis::{
        CFGAnalyses, FunctionAnalysisList, FunctionAnalysisManager, ModuleAnalysisManager,
    };
    use crate::dominator_tree::DominatorTreeAnalysis;
    use crate::instruction::InstructionView;
    use crate::pass_access::{Inspect, PatchBody, ReshapeCfg, RewriteModule};
    use crate::{IRBuilder, IntValue, IrError, Linkage, Module, NoFolder, Type};

    /// The `Requires` list shared by these tests: a single CFG-shaped analysis
    /// so both the infallible accessor and the preservation floors have a
    /// concrete member to check against.
    type Reqs = (DominatorTreeAnalysis,);

    /// Read-only [`FnCx`] over an [`Inspect`] rung reads its prefetched analysis
    /// and reports everything preserved. (Inspect has no `.mutate()`; that its
    /// absence fails to compile is the compile-fail lock — here we exercise the
    /// read + `done()` path.) llvmkit-specific capability-context lock (no
    /// upstream analog: LLVM pass contexts are untyped `Function&` + `FAM&`).
    #[test]
    fn inspect_cx_reads_analysis_and_reports_all() -> Result<(), IrError> {
        Module::with_new("inspect-cx", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
            let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");
            let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
            b.build_ret(i32_ty.const_int(1_u32))?;

            let function = FunctionView::from(f);
            let mut fam = FunctionAnalysisManager::new();
            Reqs::prefetch(&mut fam, function)?;
            let results = Reqs::collect(&fam, function)?;

            let cx: FnCx<'_, '_, '_, _, Inspect, Reqs> = FnCx::new((), function, results);

            // The prefetched analysis is reachable through the infallible
            // accessor, and the entry block is reachable from itself.
            let dt = cx.analysis::<DominatorTreeAnalysis, _>();
            let entry_view = function
                .entry_block()
                .expect("definition has an entry block");
            assert!(dt.is_reachable_from_entry(entry_view));

            // An inspect context can only report "all preserved".
            let report = cx.done();
            assert!(report.into_parts().0.are_all_preserved());
            Ok(())
        })
    }

    /// `FnCx::mutate` on a [`PatchBody`] rung yields an [`super::FnPatch`] that
    /// erases an instruction; its `done()` reports the CFG-preserved floor (the
    /// CFG set survives, an arbitrary analysis does not) — mirroring the
    /// `preserved_floor_values` checker idiom. llvmkit-specific
    /// capability-context lock (no upstream analog: LLVM pass contexts are
    /// untyped `Function&` + `FAM&`).
    #[test]
    fn patchbody_mutate_erase_reports_cfg_floor() -> Result<(), IrError> {
        Module::with_new("patch-cx", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
            let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");
            let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
            let x: IntValue<i32> = f.param(0)?.try_into()?;
            // `%dead` has no uses — a non-terminator we can erase.
            let dead = b.build_int_add(x, 1_i32, "dead")?;
            b.build_ret(x)?;

            let function = FunctionView::from(f);
            let dead_view = InstructionView::try_from(dead.as_value())?;

            let mut fam = FunctionAnalysisManager::new();
            Reqs::prefetch(&mut fam, function)?;
            let results = Reqs::collect(&fam, function)?;

            // Before mutation: entry holds `%dead` and `ret`.
            assert_eq!(
                function
                    .entry_block()
                    .expect("definition has an entry block")
                    .instruction_count(),
                2
            );

            let cx: FnCx<'_, '_, '_, _, PatchBody, Reqs> = FnCx::new(&m, function, results);
            let patch = cx.mutate();
            patch.erase(
                &dead_view
                    .as_non_terminator()
                    .expect("dead add is a non-terminator"),
            );

            // After mutation: only `ret` remains.
            assert_eq!(
                function
                    .entry_block()
                    .expect("definition has an entry block")
                    .instruction_count(),
                1
            );

            let report = patch.done();
            let pa = report.into_parts().0;
            let checker = pa.checker::<DominatorTreeAnalysis>();
            // CFG analyses survive an in-block edit; an arbitrary analysis does not.
            assert!(checker.preserved_set::<CFGAnalyses>());
            assert!(!checker.preserved());
            Ok(())
        })
    }

    /// A terminator cannot be narrowed to a [`NonTerminator`], which is the
    /// only thing [`super::FnPatch::erase`] accepts — so a terminator-erase is
    /// unrepresentable (a compile error, pinned by the `patchbody_cannot_erase_terminator`
    /// trybuild fixture) rather than a runtime rejection. This is what keeps the
    /// CFG-preserved floor sound. llvmkit-specific capability-context lock.
    #[test]
    fn terminator_does_not_narrow_to_non_terminator() -> Result<(), IrError> {
        Module::with_new("patch-term", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
            let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");
            let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
            let x: IntValue<i32> = f.param(0)?.try_into()?;
            b.build_ret(x)?;

            let function = FunctionView::from(f);
            let terminator = function
                .entry_block()
                .expect("definition has an entry block")
                .as_basic_block()
                .terminator()
                .expect("block is terminated by the ret");

            // The `ret` refuses to narrow, so it can never reach `erase`.
            assert!(terminator.as_non_terminator().is_none());
            Ok(())
        })
    }

    /// After `mutate()`, the [`super::FnPatch`] still resolves the prefetched
    /// analysis — proving the mutator carries the results into mutation.
    /// llvmkit-specific capability-context lock (no upstream analog).
    #[test]
    fn patchbody_analysis_available_during_mutation() -> Result<(), IrError> {
        Module::with_new("patch-analysis", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
            let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");
            let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
            let x: IntValue<i32> = f.param(0)?.try_into()?;
            b.build_ret(x)?;

            let function = FunctionView::from(f);
            let mut fam = FunctionAnalysisManager::new();
            Reqs::prefetch(&mut fam, function)?;
            let results = Reqs::collect(&fam, function)?;

            let cx: FnCx<'_, '_, '_, _, PatchBody, Reqs> = FnCx::new(&m, function, results);
            let patch = cx.mutate();

            // The prefetched dominator tree is still reachable mid-mutation.
            let dt = patch.analysis::<DominatorTreeAnalysis, _>();
            let entry_view = function
                .entry_block()
                .expect("definition has an entry block");
            assert!(dt.is_reachable_from_entry(entry_view));
            Ok(())
        })
    }

    /// `body_instructions()` yields every non-terminator once in program order,
    /// never the terminator, and erasing the yielded instruction mid-iteration
    /// does not disturb the walk (early-increment). llvmkit-specific pass-authoring
    /// primitive (no upstream analog: LLVM's make_early_inc_range is untyped).
    #[test]
    fn body_instructions_early_inc_erase_of_yielded() -> Result<(), IrError> {
        Module::with_new("body-cursor", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
            let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");
            let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
            let x: IntValue<i32> = f.param(0)?.try_into()?;
            let _d1 = b.build_int_add(x, 1_i32, "d1")?;
            let _d2 = b.build_int_add(x, 2_i32, "d2")?;
            b.build_ret(x)?;

            let function = FunctionView::from(f);
            let cx: FnCx<'_, '_, '_, _, PatchBody, ()> = FnCx::new(&m, function, ());
            let patch = cx.mutate();

            // Two non-terminators visited; the ret is never yielded. Erase each as
            // it is yielded — early-inc means the walk is unperturbed.
            let mut count = 0;
            let names: Vec<_> = patch
                .body_instructions()
                .map(|nt| {
                    count += 1;
                    let name = nt.as_view().as_value().name();
                    patch.erase(&nt); // erase the yielded instruction
                    name
                })
                .collect();
            assert_eq!(count, 2);
            assert_eq!(names.len(), 2);
            // Both dead adds gone; only ret remains.
            assert_eq!(function.entry_block().expect("def").instruction_count(), 1);
            let _ = patch.done();
            Ok(())
        })
    }

    /// An [`super::FnReshape`] (`ReshapeCfg` rung) `done()` reports nothing
    /// preserved — not even the CFG set — *once it has actually mutated* (the
    /// dirty flag is set). llvmkit-specific capability-context lock (no upstream
    /// analog).
    #[test]
    fn reshape_cfg_floor_is_none() -> Result<(), IrError> {
        Module::with_new("reshape-cx", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
            let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");
            let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
            let dead = b.build_int_add::<i32, _, _, _>(
                i32_ty.const_int(1_u32),
                i32_ty.const_int(2_u32),
                "dead",
            )?;
            b.build_ret(i32_ty.const_int(0_u32))?;

            let function = FunctionView::from(f);
            let mut fam = FunctionAnalysisManager::new();
            Reqs::prefetch(&mut fam, function)?;
            let results = Reqs::collect(&fam, function)?;

            let cx: FnCx<'_, '_, '_, _, ReshapeCfg, Reqs> = FnCx::new(&m, function, results);
            let reshape = cx.mutate();
            // Erase the dead instruction so the dirty flag is set; only then
            // does `done()` report the ReshapeCfg floor.
            let dead_view = InstructionView::try_from(dead.as_value())?;
            reshape.erase(
                &dead_view
                    .as_non_terminator()
                    .expect("dead add is a non-terminator"),
            );
            let report = reshape.done();
            let pa = report.into_parts().0;
            let checker = pa.checker::<DominatorTreeAnalysis>();
            assert!(!checker.preserved());
            assert!(!checker.preserved_set::<CFGAnalyses>());
            Ok(())
        })
    }

    /// A witnessed no-op `ReshapeCfg` run — `mutate()` then `done()` with no
    /// edit — reports everything preserved (the dirty flag saw nothing), so the
    /// mutating rung's floor is *not* forced on a run that changed nothing.
    #[test]
    fn reshape_cfg_noop_preserves_everything() -> Result<(), IrError> {
        Module::with_new("reshape-noop", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
            let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");
            let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
            b.build_ret(i32_ty.const_int(0_u32))?;

            let function = FunctionView::from(f);
            let mut fam = FunctionAnalysisManager::new();
            Reqs::prefetch(&mut fam, function)?;
            let results = Reqs::collect(&fam, function)?;

            let cx: FnCx<'_, '_, '_, _, ReshapeCfg, Reqs> = FnCx::new(&m, function, results);
            let report = cx.mutate().done();
            let pa = report.into_parts().0;
            assert!(pa.checker::<DominatorTreeAnalysis>().preserved());
            assert!(
                pa.checker::<DominatorTreeAnalysis>()
                    .preserved_set::<CFGAnalyses>()
            );
            Ok(())
        })
    }

    /// `split_block` records its own CFG-edge decomposition as witnessed
    /// [`CfgUpdate`](crate::CfgUpdate)s: the split moves the block's terminator
    /// — and thus every out-edge — into the fresh block, so each `block → succ`
    /// edge is logged as a delete paired with a `new_block → succ` insert. The
    /// `block → new_block` edge is the caller's to wire (through a new
    /// terminator), so it is deliberately absent from this method's log.
    /// llvmkit-specific witnessed-preservation plumbing (no upstream analog:
    /// LLVM's `DomTreeUpdater` is hand-fed its updates).
    #[test]
    fn split_block_records_edge_decomposition() -> Result<(), IrError> {
        use crate::CfgUpdate;
        Module::with_new("reshape-cfgupdate", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
            let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");
            let next = f.append_basic_block(&m, "next");
            // Ids captured up front — the block handles are consumed by the
            // builders below.
            let entry_id = entry.as_value().id;
            let next_id = next.as_value().id;

            // entry: %x = add 1, 2 ; br label %next
            let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
            let _x = b.build_int_add::<i32, _, _, _>(
                i32_ty.const_int(1_u32),
                i32_ty.const_int(2_u32),
                "x",
            )?;
            b.build_br(next.label())?;
            // next: ret 0
            let b2 = IRBuilder::new_for::<i32>(&m).position_at_end(next);
            b2.build_ret(i32_ty.const_int(0_u32))?;

            let function = FunctionView::from(f);
            let mut fam = FunctionAnalysisManager::new();
            Reqs::prefetch(&mut fam, function)?;
            let results = Reqs::collect(&fam, function)?;

            let cx: FnCx<'_, '_, '_, _, ReshapeCfg, Reqs> = FnCx::new(&m, function, results);
            let reshape = cx.mutate();

            // Nothing recorded before any structural edit.
            assert!(reshape.pending_cfg_updates().is_empty());

            // Split the entry before its terminator: `br next` (with its
            // out-edge) moves into the fresh block.
            let entry_view = function
                .entry_block()
                .expect("definition has an entry block");
            let terminator = entry_view
                .as_basic_block()
                .terminator()
                .expect("entry is terminated by the br");
            let new_block = reshape.split_block(&entry_view, &terminator, "entry.split")?;
            let new_id = new_block.as_value().id;

            // Exactly the rewiring: entry loses `→ next`, the new block gains it.
            assert_eq!(
                reshape.pending_cfg_updates(),
                vec![
                    CfgUpdate::delete(entry_id, next_id),
                    CfgUpdate::insert(new_id, next_id),
                ],
            );
            Ok(())
        })
    }

    /// [`super::FnReshape::analysis_repaired`] returns a CFG analysis rebuilt
    /// from the *current* (post-edit) CFG, not the stale cached one. Splitting
    /// the entry before its terminator moves the `entry → next` edge into a
    /// fresh block that nothing yet flows into, so `next` becomes unreachable —
    /// a fact the pre-edit cached tree still records as reachable, and the
    /// repaired tree correctly reflects. llvmkit-specific witnessed-preservation
    /// plumbing (no upstream analog).
    #[test]
    fn analysis_repaired_reflects_the_edit() -> Result<(), IrError> {
        Module::with_new("reshape-repaired", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
            let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");
            let next = f.append_basic_block(&m, "next");
            let next_label = next.label();

            // entry: %x = add 1, 2 ; br label %next    next: ret 0
            let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
            let _x = b.build_int_add::<i32, _, _, _>(
                i32_ty.const_int(1_u32),
                i32_ty.const_int(2_u32),
                "x",
            )?;
            b.build_br(next.label())?;
            let b2 = IRBuilder::new_for::<i32>(&m).position_at_end(next);
            b2.build_ret(i32_ty.const_int(0_u32))?;

            let function = FunctionView::from(f);
            let mut fam = FunctionAnalysisManager::new();
            Reqs::prefetch(&mut fam, function)?;

            // Pre-edit cached tree: `next` is reachable (entry → next).
            assert!(
                fam.get_cached_result::<DominatorTreeAnalysis, _>(function)
                    .expect("dom tree was prefetched")
                    .is_reachable_from_entry(next_label)
            );

            let results = Reqs::collect(&fam, function)?;
            let cx: FnCx<'_, '_, '_, _, ReshapeCfg, Reqs> = FnCx::new(&m, function, results);
            let mut reshape = cx.mutate();

            let entry_view = function
                .entry_block()
                .expect("definition has an entry block");
            let terminator = entry_view
                .as_basic_block()
                .terminator()
                .expect("entry is terminated by the br");
            let _new = reshape.split_block(&entry_view, &terminator, "entry.split")?;

            // The repaired tree recomputed from the current CFG, in which `next`
            // is no longer reachable — proving it is not the stale cache.
            let dt = reshape.analysis_repaired::<DominatorTreeAnalysis, _>();
            assert!(!dt.is_reachable_from_entry(next_label));
            Ok(())
        })
    }

    /// Read-only [`ModCx`] over an [`Inspect`] rung reads a per-function analysis
    /// (empty module `Requires`, so the fallible per-function accessor is the
    /// path, mirroring `pass_manager::tests::LogModulePass`) and reports
    /// everything preserved. Inspect has no `.mutate()`. llvmkit-specific
    /// capability-context lock (no upstream analog: LLVM module-pass contexts are
    /// untyped `Module&` + `MAM&`).
    #[test]
    fn inspect_modcx_reads_analysis_and_reports_all() -> Result<(), IrError> {
        Module::with_new("inspect-modcx", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
            let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");
            let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
            b.build_ret(i32_ty.const_int(1_u32))?;

            let module = m.as_view();
            let function = FunctionView::from(f);

            let mam = ModuleAnalysisManager::new();
            let mut fam = FunctionAnalysisManager::new();
            // `function_analysis` is deliberately fallible, so — like every other
            // caller of this accessor — the analysis must be registered first.
            fam.register_pass(DominatorTreeAnalysis);

            let mut cx: ModCx<'_, '_, '_, '_, _, Inspect, ()> =
                ModCx::new(module, (), (), &mam, &mut fam);

            // The per-function analysis is reachable through the fallible
            // accessor, and the entry block is reachable from itself.
            let dt = cx.function_analysis::<DominatorTreeAnalysis>(function)?;
            let entry_view = function
                .entry_block()
                .expect("definition has an entry block");
            assert!(dt.is_reachable_from_entry(entry_view));

            // An inspect module context can only report "all preserved".
            let report = cx.done();
            assert!(report.into_pa().are_all_preserved());
            Ok(())
        })
    }

    /// A [`RewriteModule`] [`ModCx`] transitions through `mutate()` into a
    /// [`super::ModRewrite`], adds a global straight through the raw module token
    /// (no sugar), and its `done()` reports the `none()` floor — nothing
    /// preserved, not even the CFG set. llvmkit-specific capability-context lock
    /// (no upstream analog).
    #[test]
    fn rewrite_module_mutate_reports_none_floor() -> Result<(), IrError> {
        Module::with_new("rewrite-modcx", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
            let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");
            let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
            b.build_ret(i32_ty.const_int(0_u32))?;

            let module = m.as_view();
            let mam = ModuleAnalysisManager::new();
            let mut fam = FunctionAnalysisManager::new();

            // No globals yet.
            assert_eq!(module.iter_globals().len(), 0);

            let cx: ModCx<'_, '_, '_, '_, _, RewriteModule, ()> =
                ModCx::new(module, &m, (), &mam, &mut fam);
            let r = cx.mutate();

            // Reach the module's own `add_global` directly through the token.
            r.module_mut().add_global("g", i32_ty.const_zero())?;

            // The mutation is visible on the module.
            assert_eq!(module.iter_globals().len(), 1);

            let rep = r.done();
            let pa = rep.into_pa();
            let checker = pa.checker::<DominatorTreeAnalysis>();
            // A module rewrite preserves nothing — the heaviest rung's floor.
            assert!(!checker.preserved());
            assert!(!checker.preserved_set::<CFGAnalyses>());
            Ok(())
        })
    }

    /// [`super::ModRewrite::for_each_function`] visits every function *definition*
    /// in module order (skipping the declaration) and hands each a
    /// [`super::FnPatch`] that erases the function's dead instruction.
    /// llvmkit-specific capability-context lock (no upstream analog).
    #[test]
    fn for_each_function_visits_defs_and_can_patch() -> Result<(), IrError> {
        Module::with_new("foreach-modcx", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);

            // Definition `f1` with a dead `add` we can erase.
            let f1 = m.add_function::<i32, _>("f1", fn_ty, Linkage::External)?;
            let e1 = f1.append_basic_block(&m, "entry");
            let b1 = IRBuilder::new_for::<i32>(&m).position_at_end(e1);
            let x1: IntValue<i32> = f1.param(0)?.try_into()?;
            let dead1 = b1.build_int_add(x1, 1_i32, "dead")?;
            b1.build_ret(x1)?;

            // Definition `f2`, likewise.
            let f2 = m.add_function::<i32, _>("f2", fn_ty, Linkage::External)?;
            let e2 = f2.append_basic_block(&m, "entry");
            let b2 = IRBuilder::new_for::<i32>(&m).position_at_end(e2);
            let x2: IntValue<i32> = f2.param(0)?.try_into()?;
            let dead2 = b2.build_int_add(x2, 1_i32, "dead")?;
            b2.build_ret(x2)?;

            // A declaration (no body) — must be skipped.
            let decl = m.add_function::<i32, _>("ext", fn_ty, Linkage::External)?;

            let fv1 = FunctionView::from(f1);
            let fv2 = FunctionView::from(f2);
            let decl_view = FunctionView::from(decl);
            let dead1_view = InstructionView::try_from(dead1.as_value())?;
            let dead2_view = InstructionView::try_from(dead2.as_value())?;

            // Each def starts with `dead` + `ret`.
            assert_eq!(fv1.entry_block().expect("def").instruction_count(), 2);
            assert_eq!(fv2.entry_block().expect("def").instruction_count(), 2);

            let module = m.as_view();
            let mam = ModuleAnalysisManager::new();
            let mut fam = FunctionAnalysisManager::new();

            let dead_by_fn = [(fv1, dead1_view), (fv2, dead2_view)];

            let cx: ModCx<'_, '_, '_, '_, _, RewriteModule, ()> =
                ModCx::new(module, &m, (), &mam, &mut fam);
            let mut r = cx.mutate();

            let mut visited: Vec<FunctionView<'_, _>> = Vec::new();
            r.for_each_function::<PatchBody>(|p| {
                let fv = p.function();
                visited.push(fv);
                for (f, dead) in &dead_by_fn {
                    if *f == fv {
                        p.erase(&dead.as_non_terminator().expect("dead is a non-terminator"));
                    }
                }
                Ok(())
            })?;

            // Both definitions were visited; the declaration was skipped.
            assert_eq!(visited.len(), 2);
            assert!(visited.contains(&fv1));
            assert!(visited.contains(&fv2));
            assert!(!visited.contains(&decl_view));

            // Each visited def now holds only `ret` — the dead `add` is gone.
            assert_eq!(fv1.entry_block().expect("def").instruction_count(), 1);
            assert_eq!(fv2.entry_block().expect("def").instruction_count(), 1);
            Ok(())
        })
    }

    /// A straight-line chain `a -> b -> c` of dead instructions (each only used
    /// by the next) drains to the transitive-dead fixpoint in ONE seed+drain,
    /// with no restart scan: the pre-seeded LIFO visits uses before defs, so
    /// erasing each dead instruction as it surfaces clears the whole chain in a
    /// single pass. (The operand-push cascade is exercised directly by
    /// `erase_pushes_operands_onto_active_worklist`; here the point is the
    /// fixpoint-in-one-drain, not the re-push.) llvmkit-specific pass-authoring
    /// primitive (no upstream analog).
    #[test]
    fn worklist_operand_cascade_reaches_fixpoint() -> Result<(), IrError> {
        Module::with_new("wl-cascade", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
            let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");
            let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
            let x: IntValue<i32> = f.param(0)?.try_into()?;
            // a = x+1 (used by b), b = a+1 (used by c), c = b+1 (unused/dead).
            let a = b.build_int_add(x, 1_i32, "a")?;
            let bb = b.build_int_add(a, 1_i32, "b")?;
            let _c = b.build_int_add(bb, 1_i32, "c")?;
            b.build_ret(x)?;

            let function = FunctionView::from(f);
            let cx: FnCx<'_, '_, '_, _, PatchBody, ()> = FnCx::new(&m, function, ());
            let patch = cx.mutate();

            // Seed + drain once: only `c` is dead initially, but erasing it makes
            // `b` dead, then `a`. One drain removes all three.
            let scope = patch.worklist();
            while let Some(inst) = scope.next() {
                if crate::dce::is_trivially_dead(&inst.as_view()) {
                    patch.erase(&inst);
                }
            }
            drop(scope);
            // Only the ret survives.
            assert_eq!(function.entry_block().expect("def").instruction_count(), 1);
            let _ = patch.done();
            Ok(())
        })
    }

    /// `erase` on an active worklist re-pushes the erased instruction's
    /// operand-defining instructions — the cascade's engine. Directly
    /// discriminating: with the seed already fully drained, the erased `%b`'s
    /// instruction operand `%a` resurfaces ONLY because `erase` re-pushed it (a
    /// non-instruction operand — the constant `1` — is skipped by the
    /// panic-safe pop). Deleting the push loop in `erase` makes the final
    /// `scope.next()` return `None` and this test fail. llvmkit-specific
    /// pass-authoring primitive (no upstream analog).
    #[test]
    fn erase_pushes_operands_onto_active_worklist() -> Result<(), IrError> {
        Module::with_new("wl-erase-push", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
            let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");
            let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
            let x: IntValue<i32> = f.param(0)?.try_into()?;
            // a = x+1 ; b = a+1 (so b's operand `a` IS an instruction) ; ret x.
            let a = b.build_int_add(x, 1_i32, "a")?;
            let bb = b.build_int_add(a, 1_i32, "b")?;
            b.build_ret(x)?;
            let a_id = a.as_value().id;
            let b_id = bb.as_value().id;

            let function = FunctionView::from(f);
            let cx: FnCx<'_, '_, '_, _, PatchBody, ()> = FnCx::new(&m, function, ());
            let patch = cx.mutate();

            // Seed [a, b]; drain the seed WITHOUT erasing, saving `b`'s handle.
            // LIFO pops `b` first, then `a`; then the worklist is empty and both
            // instructions are still attached.
            let scope = patch.worklist();
            let first = scope.next().expect("seed pops b first (LIFO)");
            assert_eq!(first.as_value().id, b_id, "LIFO seed order: b before a");
            let second = scope.next().expect("seed pops a second");
            assert_eq!(second.as_value().id, a_id);
            assert!(scope.next().is_none(), "seed fully drained");

            // Erase `b` through the active worklist: this must push `b`'s
            // operand defs, including the instruction `%a` (the constant `1` is
            // skipped by the panic-safe pop).
            patch.erase(&first);

            // `%a` resurfaces ONLY because `erase` re-pushed it. Without the
            // push loop this is `None`.
            let resurfaced = scope.next();
            assert_eq!(
                resurfaced
                    .expect("a re-pushed by erase's operand cascade")
                    .as_value()
                    .id,
                a_id,
            );
            drop(scope);
            let _ = patch.done();
            Ok(())
        })
    }

    /// A second [`super::FnPatch::worklist`] call while an earlier
    /// [`super::WorklistScope`] is still live panics rather than silently
    /// clobbering the shared slot (which would leave the outer scope inert and
    /// its `Drop` reset the inner's worklist). llvmkit-specific pass-authoring
    /// guardrail (no upstream analog).
    #[test]
    #[should_panic(expected = "already active")]
    fn nested_worklist_scopes_panic() {
        Module::with_new("wl-nested", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
            let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");
            let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
            let x: IntValue<i32> = f.param(0)?.try_into()?;
            let _a = b.build_int_add(x, 1_i32, "a")?;
            b.build_ret(x)?;

            let function = FunctionView::from(f);
            let cx: FnCx<'_, '_, '_, _, PatchBody, ()> = FnCx::new(&m, function, ());
            let patch = cx.mutate();

            let _s1 = patch.worklist();
            let _s2 = patch.worklist(); // must panic: a scope is already active
            Ok::<(), IrError>(())
        })
        .expect("unreachable: the second worklist() call panics first");
    }
}
