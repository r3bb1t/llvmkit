//! Minimal module/function pass managers. This mirrors LLVM's new-PM
//! sequencing and invalidation shape without textual pipelines, loop PM,
//! CGSCC, or transform libraries. Read-only managers preserve verification at
//! the type level; transform managers receive mutation capability and therefore
//! always return an unverified module that must be verified before the next
//! verified-only pipeline.

use core::marker::PhantomData;

use crate::IrResult;
use crate::analysis::{
    AllAnalysesOnFunction, AllAnalysesOnModule, FunctionAnalysisManager,
    FunctionAnalysisManagerModuleProxy, ModuleAnalysisManager, PreservedAnalyses,
};
use crate::module::{Brand, Module, ModuleBrand, Unverified, Verified};
use crate::pass_context::{
    FunctionPassContext, FunctionView, ModulePassContext, ReadOnlyFunctionPassContext,
    ReadOnlyModulePassContext,
};
use crate::pass_instrumentation::PassInstrumentationCallbacks;

/// Pass-manager effect for pipelines that never receive mutation capability.
pub enum PreservesVerification {}

/// Pass-manager effect for pipelines that may mutate IR.
pub enum MutatesIr {}

mod effect_sealed {
    pub trait Sealed {}

    impl Sealed for super::PreservesVerification {}
    impl Sealed for super::MutatesIr {}
}

/// Effect marker implemented by pass-manager verification states.
pub trait ModulePassEffect: effect_sealed::Sealed {
    #[doc(hidden)]
    type FunctionContext<'pm, 'ctx, B: ModuleBrand + 'ctx>
    where
        'ctx: 'pm;
    #[doc(hidden)]
    type ModuleContext<'pm, 'ctx, B: ModuleBrand + 'ctx>
    where
        'ctx: 'pm;
}

impl ModulePassEffect for PreservesVerification {
    type FunctionContext<'pm, 'ctx, B: ModuleBrand + 'ctx>
        = ReadOnlyFunctionPassContext<'pm, 'ctx, B>
    where
        'ctx: 'pm;

    type ModuleContext<'pm, 'ctx, B: ModuleBrand + 'ctx>
        = ReadOnlyModulePassContext<'pm, 'ctx, B>
    where
        'ctx: 'pm;
}

impl ModulePassEffect for MutatesIr {
    type FunctionContext<'pm, 'ctx, B: ModuleBrand + 'ctx>
        = FunctionPassContext<'pm, 'ctx, B>
    where
        'ctx: 'pm;

    type ModuleContext<'pm, 'ctx, B: ModuleBrand + 'ctx>
        = ModulePassContext<'pm, 'ctx, B>
    where
        'ctx: 'pm;
}

/// A read-only pass over one function body.
pub trait ReadOnlyFunctionPass<'ctx, B: ModuleBrand = Brand<'ctx>> {
    fn run(
        &mut self,
        cx: &mut ReadOnlyFunctionPassContext<'_, 'ctx, B>,
    ) -> IrResult<PreservedAnalyses>;

    fn is_required(&self) -> bool {
        false
    }
}

/// A transform-capable pass over one function body.
pub trait FunctionPass<'ctx, B: ModuleBrand = Brand<'ctx>> {
    fn run(&mut self, cx: &mut FunctionPassContext<'_, 'ctx, B>) -> IrResult<PreservedAnalyses>;

    fn is_required(&self) -> bool {
        false
    }
}

/// A read-only pass over one module.
pub trait ReadOnlyModulePass<'ctx, B: ModuleBrand = Brand<'ctx>> {
    fn run(
        &mut self,
        cx: &mut ReadOnlyModulePassContext<'_, 'ctx, B>,
    ) -> IrResult<PreservedAnalyses>;

    fn is_required(&self) -> bool {
        false
    }
}

/// A transform-capable pass over one module.
pub trait ModulePass<'ctx, B: ModuleBrand = Brand<'ctx>> {
    fn run(&mut self, cx: &mut ModulePassContext<'_, 'ctx, B>) -> IrResult<PreservedAnalyses>;

    fn is_required(&self) -> bool {
        false
    }
}

trait ErasedFunctionPass<'ctx, B: ModuleBrand + 'ctx, E: ModulePassEffect> {
    fn run(&mut self, cx: &mut E::FunctionContext<'_, 'ctx, B>) -> IrResult<PreservedAnalyses>;

    fn name(&self) -> &'static str;
    fn is_required(&self) -> bool;
}

struct FunctionPassModel<P> {
    pass: P,
}

impl<'ctx, B, P> ErasedFunctionPass<'ctx, B, PreservesVerification> for FunctionPassModel<P>
where
    B: ModuleBrand + 'ctx,
    P: ReadOnlyFunctionPass<'ctx, B>,
{
    fn run(
        &mut self,
        cx: &mut ReadOnlyFunctionPassContext<'_, 'ctx, B>,
    ) -> IrResult<PreservedAnalyses> {
        self.pass.run(cx)
    }

    fn name(&self) -> &'static str {
        std::any::type_name::<P>()
    }

    fn is_required(&self) -> bool {
        self.pass.is_required()
    }
}

impl<'ctx, B, P> ErasedFunctionPass<'ctx, B, MutatesIr> for FunctionPassModel<P>
where
    B: ModuleBrand + 'ctx,
    P: FunctionPass<'ctx, B>,
{
    fn run(&mut self, cx: &mut FunctionPassContext<'_, 'ctx, B>) -> IrResult<PreservedAnalyses> {
        self.pass.run(cx)
    }

    fn name(&self) -> &'static str {
        std::any::type_name::<P>()
    }

    fn is_required(&self) -> bool {
        self.pass.is_required()
    }
}

trait ErasedModulePass<'ctx, B: ModuleBrand + 'ctx, E: ModulePassEffect> {
    fn run(&mut self, cx: &mut E::ModuleContext<'_, 'ctx, B>) -> IrResult<PreservedAnalyses>;

    fn name(&self) -> &'static str;
    fn is_required(&self) -> bool;
}

struct ModulePassModel<P> {
    pass: P,
}

impl<'ctx, B, P> ErasedModulePass<'ctx, B, PreservesVerification> for ModulePassModel<P>
where
    B: ModuleBrand + 'ctx,
    P: ReadOnlyModulePass<'ctx, B>,
{
    fn run(
        &mut self,
        cx: &mut ReadOnlyModulePassContext<'_, 'ctx, B>,
    ) -> IrResult<PreservedAnalyses> {
        self.pass.run(cx)
    }

    fn name(&self) -> &'static str {
        std::any::type_name::<P>()
    }

    fn is_required(&self) -> bool {
        self.pass.is_required()
    }
}

impl<'ctx, B, P> ErasedModulePass<'ctx, B, MutatesIr> for ModulePassModel<P>
where
    B: ModuleBrand + 'ctx,
    P: ModulePass<'ctx, B>,
{
    fn run(&mut self, cx: &mut ModulePassContext<'_, 'ctx, B>) -> IrResult<PreservedAnalyses> {
        self.pass.run(cx)
    }

    fn name(&self) -> &'static str {
        std::any::type_name::<P>()
    }

    fn is_required(&self) -> bool {
        self.pass.is_required()
    }
}

/// Sequence of function passes.
pub struct FunctionPassManager<
    'ctx,
    B: ModuleBrand + 'ctx = Brand<'ctx>,
    E: ModulePassEffect + 'ctx = MutatesIr,
> {
    passes: Vec<Box<dyn ErasedFunctionPass<'ctx, B, E> + 'ctx>>,
    instrumentation: Option<PassInstrumentationCallbacks>,
    _brand: PhantomData<fn(B) -> B>,
    _effect: PhantomData<E>,
}

impl<'ctx, B, E> FunctionPassManager<'ctx, B, E>
where
    B: ModuleBrand + 'ctx,
    E: ModulePassEffect + 'ctx,
{
    fn empty() -> Self {
        Self {
            passes: Vec::new(),
            instrumentation: None,
            _brand: PhantomData,
            _effect: PhantomData,
        }
    }

    pub fn set_instrumentation(&mut self, callbacks: PassInstrumentationCallbacks) {
        self.instrumentation = Some(callbacks);
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPassManager<'ctx, B, PreservesVerification> {
    pub fn new_read_only() -> Self {
        Self::empty()
    }

    pub fn add_pass<P>(&mut self, pass: P)
    where
        P: ReadOnlyFunctionPass<'ctx, B> + 'ctx,
    {
        self.passes.push(Box::new(FunctionPassModel { pass }));
    }

    pub fn run<F>(
        &mut self,
        module: Module<'ctx, B, Verified>,
        function: F,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Module<'ctx, B, Verified>>
    where
        F: Into<FunctionView<'ctx, B>>,
    {
        self.run_read_only_inner(function.into(), None, fam)?;
        Ok(module)
    }

    pub(crate) fn run_read_only_inner(
        &mut self,
        function: FunctionView<'ctx, B>,
        mam: Option<&ModuleAnalysisManager<'ctx, B>>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<PreservedAnalyses> {
        let mut preserved = PreservedAnalyses::all();
        for pass in &mut self.passes {
            let should_run = self.instrumentation.as_ref().is_none_or(|callbacks| {
                callbacks.run_before_pass(pass.name(), pass.is_required()) || pass.is_required()
            });
            if !should_run {
                continue;
            }
            let mut cx = ReadOnlyFunctionPassContext::new(function, mam, fam);
            let pass_pa = pass.run(&mut cx)?;
            if let Some(callbacks) = &self.instrumentation {
                callbacks.run_after_pass(pass.name(), &pass_pa);
            }
            cx.function_analysis_manager_mut()
                .invalidate(function.as_function(), &pass_pa)?;
            preserved.intersect(pass_pa);
        }
        preserved.preserve_set::<AllAnalysesOnFunction>();
        Ok(preserved)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPassManager<'ctx, B, MutatesIr> {
    pub fn new_transform() -> Self {
        Self::empty()
    }

    pub fn add_pass<P>(&mut self, pass: P)
    where
        P: FunctionPass<'ctx, B> + 'ctx,
    {
        self.passes.push(Box::new(FunctionPassModel { pass }));
    }

    pub fn run<F>(
        &mut self,
        module: Module<'ctx, B, Verified>,
        function: F,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Module<'ctx, B, Unverified>>
    where
        F: Into<FunctionView<'ctx, B>>,
    {
        let module = module.unverify();
        self.run_transform_inner(&module, function.into(), None, fam)?;
        Ok(module)
    }

    pub(crate) fn run_transform_inner(
        &mut self,
        module: &Module<'ctx, B, Unverified>,
        function: FunctionView<'ctx, B>,
        mam: Option<&ModuleAnalysisManager<'ctx, B>>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<PreservedAnalyses> {
        let mut preserved = PreservedAnalyses::all();
        for pass in &mut self.passes {
            let should_run = self.instrumentation.as_ref().is_none_or(|callbacks| {
                callbacks.run_before_pass(pass.name(), pass.is_required()) || pass.is_required()
            });
            if !should_run {
                continue;
            }
            let mut cx = FunctionPassContext::new(module, function, mam, fam);
            let pass_pa = pass.run(&mut cx)?;
            if let Some(callbacks) = &self.instrumentation {
                callbacks.run_after_pass(pass.name(), &pass_pa);
            }
            cx.analysis_manager_mut()
                .invalidate(function.as_function(), &pass_pa)?;
            preserved.intersect(pass_pa);
        }
        preserved.preserve_set::<AllAnalysesOnFunction>();
        Ok(preserved)
    }
}

/// Sequence of module passes.
pub struct ModulePassManager<
    'ctx,
    B: ModuleBrand + 'ctx = Brand<'ctx>,
    E: ModulePassEffect + 'ctx = MutatesIr,
> {
    passes: Vec<Box<dyn ErasedModulePass<'ctx, B, E> + 'ctx>>,
    instrumentation: Option<PassInstrumentationCallbacks>,
    _brand: PhantomData<fn(B) -> B>,
    _effect: PhantomData<E>,
}

impl<'ctx, B, E> ModulePassManager<'ctx, B, E>
where
    B: ModuleBrand + 'ctx,
    E: ModulePassEffect + 'ctx,
{
    fn empty() -> Self {
        Self {
            passes: Vec::new(),
            instrumentation: None,
            _brand: PhantomData,
            _effect: PhantomData,
        }
    }

    pub fn set_instrumentation(&mut self, callbacks: PassInstrumentationCallbacks) {
        self.instrumentation = Some(callbacks);
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> ModulePassManager<'ctx, B, PreservesVerification> {
    pub fn new_read_only() -> Self {
        Self::empty()
    }

    pub fn add_pass<P>(&mut self, pass: P)
    where
        P: ReadOnlyModulePass<'ctx, B> + 'ctx,
    {
        self.passes.push(Box::new(ModulePassModel { pass }));
    }

    pub fn run(
        &mut self,
        module: Module<'ctx, B, Verified>,
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Module<'ctx, B, Verified>> {
        let mut cx = ReadOnlyModulePassContext::new(&module, mam, fam);
        let mut preserved = PreservedAnalyses::all();
        for pass in &mut self.passes {
            let should_run = self.instrumentation.as_ref().is_none_or(|callbacks| {
                callbacks.run_before_pass(pass.name(), pass.is_required()) || pass.is_required()
            });
            if !should_run {
                continue;
            }
            let pass_pa = pass.run(&mut cx)?;
            if let Some(callbacks) = &self.instrumentation {
                callbacks.run_after_pass(pass.name(), &pass_pa);
            }
            let module_view = cx.module();
            cx.module_analysis_manager_mut()
                .invalidate(module_view, &pass_pa)?;
            cx.function_analysis_manager_mut()
                .invalidate_module(module_view, &pass_pa)?;
            preserved.intersect(pass_pa);
        }
        preserved.preserve_set::<AllAnalysesOnModule>();
        Ok(module)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> ModulePassManager<'ctx, B, MutatesIr> {
    pub fn new_transform() -> Self {
        Self::empty()
    }

    pub fn add_pass<P>(&mut self, pass: P)
    where
        P: ModulePass<'ctx, B> + 'ctx,
    {
        self.passes.push(Box::new(ModulePassModel { pass }));
    }

    pub fn run(
        &mut self,
        module: Module<'ctx, B, Verified>,
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Module<'ctx, B, Unverified>> {
        let mut cx = ModulePassContext::new(module.unverify(), mam, fam);
        let mut preserved = PreservedAnalyses::all();
        for pass in &mut self.passes {
            let should_run = self.instrumentation.as_ref().is_none_or(|callbacks| {
                callbacks.run_before_pass(pass.name(), pass.is_required()) || pass.is_required()
            });
            if !should_run {
                continue;
            }
            let pass_pa = pass.run(&mut cx)?;
            if let Some(callbacks) = &self.instrumentation {
                callbacks.run_after_pass(pass.name(), &pass_pa);
            }
            let module_view = cx.module();
            cx.module_analysis_manager_mut()
                .invalidate(module_view, &pass_pa)?;
            cx.function_analysis_manager_mut()
                .invalidate_module(module_view, &pass_pa)?;
            preserved.intersect(pass_pa);
        }
        preserved.preserve_set::<AllAnalysesOnModule>();
        Ok(cx.finish())
    }
}

/// Module pass that runs a function pass manager over every function
/// definition in module order.
pub struct ModuleToFunctionPassAdaptor<
    'ctx,
    B: ModuleBrand + 'ctx = Brand<'ctx>,
    E: ModulePassEffect + 'ctx = MutatesIr,
> {
    fpm: FunctionPassManager<'ctx, B, E>,
}

impl<'ctx, B, E> ModuleToFunctionPassAdaptor<'ctx, B, E>
where
    B: ModuleBrand + 'ctx,
    E: ModulePassEffect + 'ctx,
{
    pub fn new(fpm: FunctionPassManager<'ctx, B, E>) -> Self {
        Self { fpm }
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> ReadOnlyModulePass<'ctx, B>
    for ModuleToFunctionPassAdaptor<'ctx, B, PreservesVerification>
{
    fn run(
        &mut self,
        cx: &mut ReadOnlyModulePassContext<'_, 'ctx, B>,
    ) -> IrResult<PreservedAnalyses> {
        let mut preserved = PreservedAnalyses::all();
        for function in cx.functions() {
            if function.entry_block().is_none() {
                continue;
            }
            let (mam, fam) = cx.analysis_managers_for_function_passes();
            let function_pa = self.fpm.run_read_only_inner(function, Some(mam), fam)?;
            preserved.intersect(function_pa);
        }
        preserved.preserve_set::<AllAnalysesOnFunction>();
        preserved.preserve::<FunctionAnalysisManagerModuleProxy>();
        Ok(preserved)
    }

    fn is_required(&self) -> bool {
        true
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> ModulePass<'ctx, B>
    for ModuleToFunctionPassAdaptor<'ctx, B, MutatesIr>
{
    fn run(&mut self, cx: &mut ModulePassContext<'_, 'ctx, B>) -> IrResult<PreservedAnalyses> {
        let mut preserved = PreservedAnalyses::all();
        for function in cx.functions() {
            if function.entry_block().is_none() {
                continue;
            }
            let (module, mam, fam) = cx.module_and_analysis_managers_for_function_passes();
            let function_pa = self
                .fpm
                .run_transform_inner(module, function, Some(mam), fam)?;
            preserved.intersect(function_pa);
        }
        preserved.preserve_set::<AllAnalysesOnFunction>();
        preserved.preserve::<FunctionAnalysisManagerModuleProxy>();
        Ok(preserved)
    }

    fn is_required(&self) -> bool {
        true
    }
}
