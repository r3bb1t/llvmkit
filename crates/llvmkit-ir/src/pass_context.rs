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
//!         let i32_ty = rewrite.module_mut().i32_type();
//!         rewrite
//!             .module_mut()
//!             .add_global("marker", i32_ty.as_type(), i32_ty.const_zero())?;
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
    AnalysisSelector, FunctionAnalysis, FunctionAnalysisList, FunctionAnalysisManager,
    ModuleAnalysis, ModuleAnalysisList, ModuleAnalysisManager, ModuleAnalysisSelector,
    PreservedAnalyses,
};
use super::block_state::{Terminated, Unterminated};
use super::function::FunctionValue;
use super::instruction::{Instruction, InstructionView, NonTerminator, state};
use super::marker::{Dyn, ReturnMarker};
use super::module::{Brand, Invariant, Module, ModuleBrand, ModuleRef, ModuleView, Unverified};
use super::pass_access::{
    FnAccess, ModAccess, MutatingFn, MutatingModule, PatchBody, ReshapeCfg, RewriteModule,
};
use super::value::IsValue;

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
// Pass API v2 — function report, entry context, and mutators
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
}

impl FnReport {
    /// Wrap a driver-derived preservation set. `pub(crate)` — this is the sole
    /// construction path, so an external author can never fabricate a report
    /// that over-claims preservation. THE honesty guarantee.
    #[inline]
    pub(crate) fn from_pa(pa: PreservedAnalyses) -> Self {
        Self { pa }
    }

    /// Consume the report and yield its preservation set. The driver-facing
    /// seam ([`crate::pass_manager::run_function_pass`] reads this to drive
    /// invalidation). `pub(crate)`: the single-pass driver is its sole caller,
    /// and reading the set out of a report cannot mint a dishonest one — unlike
    /// [`Self::from_pa`].
    #[inline]
    pub(crate) fn into_pa(self) -> PreservedAnalyses {
        self.pa
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
        inst.erase_from_parent(self.module);
        self.dirty.set(true);
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
        let inst = Instruction::<state::Attached, B>::from_parts(id, self.module.module_ref());
        inst.replace_all_uses_with(self.module, replacement)?;
        self.dirty.set(true);
        Ok(())
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

/// CFG-rewriting mutator for the [`ReshapeCfg`] rung — minimal but real. It has
/// everything [`FnPatch`] exposes (by composition) **plus** at least one genuine
/// control-flow operation ([`FnReshape::split_block`], wired to
/// [`BasicBlock::split_at`]), so the rung is distinct from `FnPatch` and its
/// `done()` floor is `none()` — nothing preserved.
///
/// Only the split primitive is shipped this branch (no in-tree consumer needs
/// more). Fuller terminator surgery — rewiring branches, inserting PHIs,
/// deleting blocks — is future work; the point here is to prove the rung and its
/// empty floor, not to be exhaustive.
pub struct FnReshape<'m, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    patch: FnPatch<'m, 'r, 'ctx, B, R>,
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
        }
    }

    // `function`, `function_mut`, `analysis`, `module_mut`, `erase`, and
    // `replace_all_uses` are inherited from `FnPatch` via `Deref` (below), so
    // the read + in-block-edit surface need not be re-delegated by hand.

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
        let new_block = block
            .as_basic_block()
            .split_at(self.patch.module_mut(), before, name)?;
        self.patch.dirty.set(true);
        Ok(new_block)
    }

    /// Finish: report the [`ReshapeCfg`] preservation floor (`none()` — nothing
    /// preserved) if anything was mutated, or everything-preserved if the run
    /// was a no-op. Consumes the mutator; the no-op case is witnessed by the
    /// dirty flag.
    #[inline]
    pub fn done(self) -> FnReport {
        if self.patch.is_dirty() {
            FnReport::from_pa(<ReshapeCfg as FnAccess>::preserved_floor())
        } else {
            FnReport::from_pa(PreservedAnalyses::all())
        }
    }
}

/// A `ReshapeCfg` mutator *is* a `PatchBody` mutator plus CFG surgery, so it
/// inherits the whole in-block-edit surface (`function`/`function_mut`/
/// `analysis`/`module_mut`/`erase`/`replace_all_uses`/`is_dirty`) via `Deref`
/// instead of re-delegating each by hand. `FnReshape`'s own `done()` (the
/// `ReshapeCfg` floor) is inherent, so it shadows the derefed `FnPatch::done`.
impl<'m, 'r, 'ctx, B, R> core::ops::Deref for FnReshape<'m, 'r, 'ctx, B, R>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    'ctx: 'm,
    'ctx: 'r,
{
    type Target = FnPatch<'m, 'r, 'ctx, B, R>;
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.patch
    }
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
// Pass API v2 — module report, entry context, and mutator
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
    /// cache-peek borrows end here — the read-only query phase is over (v1
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
            assert!(report.into_pa().are_all_preserved());
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
            let pa = report.into_pa();
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
            let pa = report.into_pa();
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
            let pa = report.into_pa();
            assert!(pa.checker::<DominatorTreeAnalysis>().preserved());
            assert!(
                pa.checker::<DominatorTreeAnalysis>()
                    .preserved_set::<CFGAnalyses>()
            );
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
            r.module_mut()
                .add_global("g", i32_ty.as_type(), i32_ty.const_zero())?;

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
}
