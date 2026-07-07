//! Pass-author context types.
//!
//! The pass managers pass these narrow contexts to user passes instead of raw
//! module storage. Read-only contexts expose verified views and analysis
//! queries; transform contexts carry an unverified module capability.

use core::marker::PhantomData;

use super::BasicBlock;
use super::IrResult;
use super::analysis::{
    AnalysisSelector, FunctionAnalysis, FunctionAnalysisList, FunctionAnalysisManager,
    ModuleAnalysis, ModuleAnalysisList, ModuleAnalysisManager, ModuleAnalysisSelector,
};
use super::block_state::Terminated;
use super::function::FunctionValue;
use super::marker::{Dyn, ReturnMarker};
use super::module::{
    Brand, Invariant, Module, ModuleBrand, ModuleRef, ModuleView, Unverified, Verified,
};
use super::pass_manager::{MutatesIr, TypedPassEffect};

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

/// Context passed to a read-only function pass.
pub struct ReadOnlyFunctionPassContext<'pm, 'ctx, B: ModuleBrand = Brand<'ctx>> {
    function: FunctionView<'ctx, B>,
    mam: Option<&'pm ModuleAnalysisManager<'ctx, B>>,
    fam: &'pm mut FunctionAnalysisManager<'ctx, B>,
}

impl<'pm, 'ctx, B: ModuleBrand + 'ctx> ReadOnlyFunctionPassContext<'pm, 'ctx, B> {
    #[inline]
    pub(super) fn new(
        function: FunctionView<'ctx, B>,
        mam: Option<&'pm ModuleAnalysisManager<'ctx, B>>,
        fam: &'pm mut FunctionAnalysisManager<'ctx, B>,
    ) -> Self {
        Self { function, mam, fam }
    }

    /// Owning module view.
    #[inline]
    pub fn module(&self) -> ModuleView<'ctx, B> {
        self.function.module()
    }

    /// Read-only function view.
    #[inline]
    pub fn function(&self) -> FunctionView<'ctx, B> {
        self.function
    }

    /// Query a function analysis for this pass's function.
    #[inline]
    pub fn analysis<A>(&mut self) -> IrResult<&A::Result>
    where
        A: FunctionAnalysis<'ctx, B>,
    {
        self.fam.get_result::<A, _>(self.function)
    }

    /// Read a cached function analysis without computing it.
    #[inline]
    pub fn cached_analysis<A>(&self) -> Option<&A::Result>
    where
        A: FunctionAnalysis<'ctx, B>,
    {
        self.fam.get_cached_result::<A, _>(self.function)
    }

    /// Read a cached module analysis without computing it.
    #[inline]
    pub fn cached_module_analysis<A>(&self) -> Option<&A::Result>
    where
        A: ModuleAnalysis<'ctx, B>,
    {
        self.mam?.get_cached_result::<A, _>(self.module())
    }

    #[inline]
    pub(super) fn function_analysis_manager_mut(
        &mut self,
    ) -> &mut FunctionAnalysisManager<'ctx, B> {
        self.fam
    }
}

/// Context passed to a transform-capable function pass.
pub struct FunctionPassContext<'pm, 'ctx, B: ModuleBrand = Brand<'ctx>> {
    module: &'pm Module<'ctx, B, Unverified>,
    function: FunctionView<'ctx, B>,
    mam: Option<&'pm ModuleAnalysisManager<'ctx, B>>,
    fam: &'pm mut FunctionAnalysisManager<'ctx, B>,
}

impl<'pm, 'ctx, B: ModuleBrand + 'ctx> FunctionPassContext<'pm, 'ctx, B> {
    #[inline]
    pub(super) fn new(
        module: &'pm Module<'ctx, B, Unverified>,
        function: FunctionView<'ctx, B>,
        mam: Option<&'pm ModuleAnalysisManager<'ctx, B>>,
        fam: &'pm mut FunctionAnalysisManager<'ctx, B>,
    ) -> Self {
        Self {
            module,
            function,
            mam,
            fam,
        }
    }

    /// Read-only module view.
    #[inline]
    pub fn module(&self) -> ModuleView<'ctx, B> {
        self.module.as_view()
    }

    /// Mutation-capable module token for saved-handle mutators.
    #[inline]
    pub fn module_mut(&self) -> &Module<'ctx, B, Unverified> {
        self.module
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

    /// Query a function analysis for this pass's function.
    #[inline]
    pub fn analysis<A>(&mut self) -> IrResult<&A::Result>
    where
        A: FunctionAnalysis<'ctx, B>,
    {
        self.fam.get_result::<A, _>(self.function)
    }

    /// Read a cached function analysis without computing it.
    #[inline]
    pub fn cached_analysis<A>(&self) -> Option<&A::Result>
    where
        A: FunctionAnalysis<'ctx, B>,
    {
        self.fam.get_cached_result::<A, _>(self.function)
    }

    /// Read a cached module analysis without computing it.
    #[inline]
    pub fn cached_module_analysis<A>(&self) -> Option<&A::Result>
    where
        A: ModuleAnalysis<'ctx, B>,
    {
        self.mam?.get_cached_result::<A, _>(self.module())
    }

    /// Function analysis manager for this module brand.
    #[inline]
    pub fn analysis_manager_mut(&mut self) -> &mut FunctionAnalysisManager<'ctx, B> {
        self.fam
    }
}

/// Context passed to a read-only module pass.
pub struct ReadOnlyModulePassContext<'pm, 'ctx, B: ModuleBrand = Brand<'ctx>> {
    module: &'pm Module<'ctx, B, Verified>,
    mam: &'pm mut ModuleAnalysisManager<'ctx, B>,
    fam: &'pm mut FunctionAnalysisManager<'ctx, B>,
}

impl<'pm, 'ctx, B: ModuleBrand + 'ctx> ReadOnlyModulePassContext<'pm, 'ctx, B> {
    #[inline]
    pub(super) fn new(
        module: &'pm Module<'ctx, B, Verified>,
        mam: &'pm mut ModuleAnalysisManager<'ctx, B>,
        fam: &'pm mut FunctionAnalysisManager<'ctx, B>,
    ) -> Self {
        Self { module, mam, fam }
    }

    /// Read-only module view.
    #[inline]
    pub fn module(&self) -> ModuleView<'ctx, B> {
        self.module.as_view()
    }

    /// Function views in declaration order.
    #[inline]
    pub fn functions(&self) -> ModuleFunctionViews<'ctx, B> {
        ModuleFunctionViews::new(self.module())
    }

    /// Query a module analysis.
    #[inline]
    pub fn module_analysis<A>(&mut self) -> IrResult<&A::Result>
    where
        A: ModuleAnalysis<'ctx, B>,
    {
        self.mam.get_result::<A>(self.module)
    }

    /// Read a cached module analysis without computing it.
    #[inline]
    pub fn cached_module_analysis<A>(&self) -> Option<&A::Result>
    where
        A: ModuleAnalysis<'ctx, B>,
    {
        self.mam.get_cached_result::<A, _>(self.module())
    }

    /// Query a function analysis for a function in this module.
    #[inline]
    pub fn function_analysis<A>(
        &mut self,
        function: FunctionView<'ctx, B>,
    ) -> crate::IrResult<&A::Result>
    where
        A: FunctionAnalysis<'ctx, B>,
    {
        self.fam.get_result::<A, _>(function)
    }

    #[inline]
    pub(super) fn module_analysis_manager_mut(&mut self) -> &mut ModuleAnalysisManager<'ctx, B> {
        self.mam
    }

    #[inline]
    pub(super) fn function_analysis_manager_mut(
        &mut self,
    ) -> &mut FunctionAnalysisManager<'ctx, B> {
        self.fam
    }

    #[inline]
    pub(super) fn analysis_managers_for_function_passes(
        &mut self,
    ) -> (
        &ModuleAnalysisManager<'ctx, B>,
        &mut FunctionAnalysisManager<'ctx, B>,
    ) {
        (self.mam, self.fam)
    }
}

/// Context passed to a transform-capable module pass.
pub struct ModulePassContext<'pm, 'ctx, B: ModuleBrand = Brand<'ctx>> {
    module: Module<'ctx, B, Unverified>,
    mam: &'pm mut ModuleAnalysisManager<'ctx, B>,
    fam: &'pm mut FunctionAnalysisManager<'ctx, B>,
}

impl<'pm, 'ctx, B: ModuleBrand + 'ctx> ModulePassContext<'pm, 'ctx, B> {
    #[inline]
    pub(super) fn new(
        module: Module<'ctx, B, Unverified>,
        mam: &'pm mut ModuleAnalysisManager<'ctx, B>,
        fam: &'pm mut FunctionAnalysisManager<'ctx, B>,
    ) -> Self {
        Self { module, mam, fam }
    }

    /// Read-only module view.
    #[inline]
    pub fn module(&self) -> ModuleView<'ctx, B> {
        self.module.as_view()
    }

    /// Mutation-capable module token.
    #[inline]
    pub fn module_mut(&self) -> &Module<'ctx, B, Unverified> {
        &self.module
    }

    /// Function views in declaration order.
    #[inline]
    pub fn functions(&self) -> ModuleFunctionViews<'ctx, B> {
        ModuleFunctionViews::new(self.module())
    }

    /// Read a cached module analysis without computing it.
    #[inline]
    pub fn cached_module_analysis<A>(&self) -> Option<&A::Result>
    where
        A: ModuleAnalysis<'ctx, B>,
    {
        self.mam.get_cached_result::<A, _>(self.module())
    }

    /// Query a function analysis for a function in this module.
    #[inline]
    pub fn function_analysis<A>(
        &mut self,
        function: FunctionView<'ctx, B>,
    ) -> crate::IrResult<&A::Result>
    where
        A: FunctionAnalysis<'ctx, B>,
    {
        self.fam.get_result::<A, _>(function)
    }

    #[inline]
    pub(super) fn module_analysis_manager_mut(&mut self) -> &mut ModuleAnalysisManager<'ctx, B> {
        self.mam
    }

    #[inline]
    pub(super) fn function_analysis_manager_mut(
        &mut self,
    ) -> &mut FunctionAnalysisManager<'ctx, B> {
        self.fam
    }

    #[inline]
    pub(super) fn module_and_analysis_managers_for_function_passes(
        &mut self,
    ) -> (
        &Module<'ctx, B, Unverified>,
        &ModuleAnalysisManager<'ctx, B>,
        &mut FunctionAnalysisManager<'ctx, B>,
    ) {
        (&self.module, self.mam, self.fam)
    }

    #[inline]
    pub(super) fn finish(self) -> Module<'ctx, B, Unverified> {
        self.module
    }
}

/// Context passed to a typed function pass. Carries only what the pass
/// declared: the per-effect module token and the prefetched `Requires`
/// results — there is no analysis manager here, so undeclared analyses are
/// unreachable rather than fallible (D1/D3; ad-hoc queries belong to the
/// erased pass path).
///
/// The module `token` (`'pm`) and the prefetched `results` (`'r`) carry
/// distinct lifetimes: the token borrows the long-lived pipeline module while
/// the results borrow the analysis manager only for the pass's `run` scope, so
/// the manager is free again for invalidation the moment `run` returns.
pub struct TypedFunctionPassContext<
    'pm,
    'r,
    'ctx,
    B: ModuleBrand,
    R: FunctionAnalysisList<'ctx, B>,
    E: TypedPassEffect,
> where
    B: 'ctx,
    'ctx: 'pm,
    'ctx: 'r,
{
    token: E::ModuleToken<'pm, 'ctx, B>,
    function: FunctionView<'ctx, B>,
    results: R::ResultRefs<'r>,
}

impl<'pm, 'r, 'ctx, B, R, E> TypedFunctionPassContext<'pm, 'r, 'ctx, B, R, E>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
    E: TypedPassEffect,
{
    #[inline]
    pub(super) fn new(
        token: E::ModuleToken<'pm, 'ctx, B>,
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

    /// Infallible access to a `Requires`-declared analysis result. The
    /// position index `I` is inferred; an undeclared analysis has no
    /// [`AnalysisSelector`] impl and fails to compile.
    #[inline]
    pub fn analysis<A, I>(&self) -> &'r A::Result
    where
        A: FunctionAnalysis<'ctx, B>,
        R: AnalysisSelector<'ctx, B, A, I>,
    {
        R::select(&self.results)
    }
}

impl<'pm, 'r, 'ctx, B, R> TypedFunctionPassContext<'pm, 'r, 'ctx, B, R, MutatesIr>
where
    B: ModuleBrand + 'ctx,
    R: FunctionAnalysisList<'ctx, B>,
{
    /// Mutation-capable module token for saved-handle mutators.
    #[inline]
    pub fn module_mut(&self) -> &'pm Module<'ctx, B, Unverified> {
        self.token
    }

    /// Mutation-capable function-body view.
    #[inline]
    pub fn function_mut(&self) -> FunctionBody<'ctx, B> {
        FunctionBody::new(self.function.as_function())
    }
}

/// Context passed to a typed module pass. Module-level `Requires` results are
/// prefetched (infallible accessor); per-function analysis queries stay
/// fallible by design -- they are inherently dynamic, mirroring upstream's
/// `FunctionAnalysisManagerModuleProxy` posture (there is no static `Requires`
/// list naming which function analyses a module pass will touch, since it may
/// visit an arbitrary subset of the module's functions).
///
/// Mirrors [`TypedFunctionPassContext`]'s two-lifetime split, plus one more:
/// the module `token` (`'pm`) borrows the long-lived pipeline module, the
/// prefetched `results` (`'r`) borrow the module analysis manager only for
/// the pass's `run` scope (so `mam` is free again for invalidation the
/// moment `run` returns), and `fam` (`'f`) is reborrowed at its own scope so
/// the caller's `&mut FunctionAnalysisManager` is likewise free again for
/// `invalidate_module` once `run` returns -- distinct from `'pm` is exactly
/// what a same-lifetime field cannot express, since `token` is `Copy` and
/// shrinks freely but a unique `&mut` borrow does not.
pub struct TypedModulePassContext<
    'pm,
    'r,
    'f,
    'ctx,
    B: ModuleBrand,
    R: ModuleAnalysisList<'ctx, B>,
    E: TypedPassEffect,
> where
    B: 'ctx,
    'ctx: 'pm,
    'ctx: 'r,
    'ctx: 'f,
{
    module: ModuleView<'ctx, B>,
    token: E::ModuleToken<'pm, 'ctx, B>,
    results: R::ResultRefs<'r>,
    fam: &'f mut FunctionAnalysisManager<'ctx, B>,
}

impl<'pm, 'r, 'f, 'ctx, B, R, E> TypedModulePassContext<'pm, 'r, 'f, 'ctx, B, R, E>
where
    B: ModuleBrand + 'ctx,
    R: ModuleAnalysisList<'ctx, B>,
    E: TypedPassEffect,
{
    #[inline]
    pub(super) fn new(
        module: ModuleView<'ctx, B>,
        token: E::ModuleToken<'pm, 'ctx, B>,
        results: R::ResultRefs<'r>,
        fam: &'f mut FunctionAnalysisManager<'ctx, B>,
    ) -> Self {
        Self {
            module,
            token,
            results,
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
    pub fn analysis<A, I>(&self) -> &'r A::Result
    where
        A: ModuleAnalysis<'ctx, B>,
        R: ModuleAnalysisSelector<'ctx, B, A, I>,
    {
        R::select(&self.results)
    }

    /// Query a function analysis for a function in this module. Deliberately
    /// dynamic (fallible): unlike module-level `Requires`, there is no static
    /// list of which functions a module pass will visit, so per-function
    /// analysis access cannot be prefetched into an infallible accessor.
    #[inline]
    pub fn function_analysis<A>(&mut self, function: FunctionView<'ctx, B>) -> IrResult<&A::Result>
    where
        A: FunctionAnalysis<'ctx, B>,
    {
        self.fam.get_result::<A, _>(function)
    }
}

impl<'pm, 'r, 'f, 'ctx, B, R> TypedModulePassContext<'pm, 'r, 'f, 'ctx, B, R, MutatesIr>
where
    B: ModuleBrand + 'ctx,
    R: ModuleAnalysisList<'ctx, B>,
{
    /// Mutation-capable module token for saved-handle mutators.
    #[inline]
    pub fn module_mut(&self) -> &'pm Module<'ctx, B, Unverified> {
        self.token
    }
}
