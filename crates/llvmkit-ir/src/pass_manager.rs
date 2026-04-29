//! Minimal module/function pass managers. This mirrors LLVM's new-PM
//! sequencing and invalidation shape without textual pipelines, loop PM,
//! CGSCC, or transform libraries.

use crate::IrResult;
use crate::analysis::{
    AllAnalysesOnFunction, AllAnalysesOnModule, FunctionAnalysisManager, ModuleAnalysisManager,
    PreservedAnalyses,
};
use crate::function::FunctionValue;
use crate::marker::Dyn;
use crate::module::Module;
use crate::pass_instrumentation::PassInstrumentationCallbacks;

/// A pass over one function.
pub trait FunctionPass<'ctx> {
    fn run(
        &mut self,
        function: FunctionValue<'ctx, Dyn>,
        fam: &mut FunctionAnalysisManager<'ctx>,
    ) -> IrResult<PreservedAnalyses>;

    fn is_required(&self) -> bool {
        false
    }
}

/// A pass over one module.
pub trait ModulePass<'ctx> {
    fn run(
        &mut self,
        module: &'ctx Module<'ctx>,
        mam: &mut ModuleAnalysisManager<'ctx>,
        fam: &mut FunctionAnalysisManager<'ctx>,
    ) -> IrResult<PreservedAnalyses>;

    fn is_required(&self) -> bool {
        false
    }
}

trait ErasedFunctionPass<'ctx> {
    fn run(
        &mut self,
        function: FunctionValue<'ctx, Dyn>,
        fam: &mut FunctionAnalysisManager<'ctx>,
    ) -> IrResult<PreservedAnalyses>;

    fn name(&self) -> &'static str;
    fn is_required(&self) -> bool;
}

struct FunctionPassModel<P> {
    pass: P,
}

impl<'ctx, P> ErasedFunctionPass<'ctx> for FunctionPassModel<P>
where
    P: FunctionPass<'ctx>,
{
    fn run(
        &mut self,
        function: FunctionValue<'ctx, Dyn>,
        fam: &mut FunctionAnalysisManager<'ctx>,
    ) -> IrResult<PreservedAnalyses> {
        self.pass.run(function, fam)
    }

    fn name(&self) -> &'static str {
        std::any::type_name::<P>()
    }

    fn is_required(&self) -> bool {
        self.pass.is_required()
    }
}

trait ErasedModulePass<'ctx> {
    fn run(
        &mut self,
        module: &'ctx Module<'ctx>,
        mam: &mut ModuleAnalysisManager<'ctx>,
        fam: &mut FunctionAnalysisManager<'ctx>,
    ) -> IrResult<PreservedAnalyses>;

    fn name(&self) -> &'static str;
    fn is_required(&self) -> bool;
}

struct ModulePassModel<P> {
    pass: P,
}

impl<'ctx, P> ErasedModulePass<'ctx> for ModulePassModel<P>
where
    P: ModulePass<'ctx>,
{
    fn run(
        &mut self,
        module: &'ctx Module<'ctx>,
        mam: &mut ModuleAnalysisManager<'ctx>,
        fam: &mut FunctionAnalysisManager<'ctx>,
    ) -> IrResult<PreservedAnalyses> {
        self.pass.run(module, mam, fam)
    }

    fn name(&self) -> &'static str {
        std::any::type_name::<P>()
    }

    fn is_required(&self) -> bool {
        self.pass.is_required()
    }
}

/// Sequence of function passes.
pub struct FunctionPassManager<'ctx> {
    passes: Vec<Box<dyn ErasedFunctionPass<'ctx> + 'ctx>>,
    instrumentation: Option<PassInstrumentationCallbacks>,
}

impl<'ctx> FunctionPassManager<'ctx> {
    pub fn new() -> Self {
        Self {
            passes: Vec::new(),
            instrumentation: None,
        }
    }

    pub fn add_pass<P>(&mut self, pass: P)
    where
        P: FunctionPass<'ctx> + 'ctx,
    {
        self.passes.push(Box::new(FunctionPassModel { pass }));
    }

    pub fn set_instrumentation(&mut self, callbacks: PassInstrumentationCallbacks) {
        self.instrumentation = Some(callbacks);
    }

    pub fn run(
        &mut self,
        function: FunctionValue<'ctx, Dyn>,
        fam: &mut FunctionAnalysisManager<'ctx>,
    ) -> IrResult<PreservedAnalyses> {
        let mut preserved = PreservedAnalyses::all();
        for pass in &mut self.passes {
            let should_run = self.instrumentation.as_ref().is_none_or(|callbacks| {
                callbacks.run_before_pass(pass.name(), pass.is_required()) || pass.is_required()
            });
            if !should_run {
                continue;
            }
            let pass_pa = pass.run(function, fam)?;
            if let Some(callbacks) = &self.instrumentation {
                callbacks.run_after_pass(pass.name(), &pass_pa);
            }
            fam.invalidate(function, &pass_pa);
            preserved.intersect(pass_pa);
        }
        preserved.preserve_set::<AllAnalysesOnFunction>();
        Ok(preserved)
    }
}

impl Default for FunctionPassManager<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// Sequence of module passes.
pub struct ModulePassManager<'ctx> {
    passes: Vec<Box<dyn ErasedModulePass<'ctx> + 'ctx>>,
    instrumentation: Option<PassInstrumentationCallbacks>,
}

impl<'ctx> ModulePassManager<'ctx> {
    pub fn new() -> Self {
        Self {
            passes: Vec::new(),
            instrumentation: None,
        }
    }

    pub fn add_pass<P>(&mut self, pass: P)
    where
        P: ModulePass<'ctx> + 'ctx,
    {
        self.passes.push(Box::new(ModulePassModel { pass }));
    }

    pub fn set_instrumentation(&mut self, callbacks: PassInstrumentationCallbacks) {
        self.instrumentation = Some(callbacks);
    }

    pub fn run(
        &mut self,
        module: &'ctx Module<'ctx>,
        mam: &mut ModuleAnalysisManager<'ctx>,
        fam: &mut FunctionAnalysisManager<'ctx>,
    ) -> IrResult<PreservedAnalyses> {
        let mut preserved = PreservedAnalyses::all();
        for pass in &mut self.passes {
            let should_run = self.instrumentation.as_ref().is_none_or(|callbacks| {
                callbacks.run_before_pass(pass.name(), pass.is_required()) || pass.is_required()
            });
            if !should_run {
                continue;
            }
            let pass_pa = pass.run(module, mam, fam)?;
            if let Some(callbacks) = &self.instrumentation {
                callbacks.run_after_pass(pass.name(), &pass_pa);
            }
            mam.invalidate(module, &pass_pa);
            preserved.intersect(pass_pa);
        }
        preserved.preserve_set::<AllAnalysesOnModule>();
        Ok(preserved)
    }
}

impl Default for ModulePassManager<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// Module pass that runs a function pass manager over every function
/// definition in module order.
pub struct ModuleToFunctionPassAdaptor<'ctx> {
    fpm: FunctionPassManager<'ctx>,
}

impl<'ctx> ModuleToFunctionPassAdaptor<'ctx> {
    pub fn new(fpm: FunctionPassManager<'ctx>) -> Self {
        Self { fpm }
    }
}

impl<'ctx> ModulePass<'ctx> for ModuleToFunctionPassAdaptor<'ctx> {
    fn run(
        &mut self,
        module: &'ctx Module<'ctx>,
        _mam: &mut ModuleAnalysisManager<'ctx>,
        fam: &mut FunctionAnalysisManager<'ctx>,
    ) -> IrResult<PreservedAnalyses> {
        let mut preserved = PreservedAnalyses::all();
        for function in module.iter_functions() {
            if function.entry_block().is_none() {
                continue;
            }
            let function_pa = self.fpm.run(function, fam)?;
            preserved.intersect(function_pa);
        }
        preserved.preserve_set::<AllAnalysesOnFunction>();
        Ok(preserved)
    }

    fn is_required(&self) -> bool {
        true
    }
}
