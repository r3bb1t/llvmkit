//! Minimal LLVM-new-PM-style analysis substrate. Mirrors the
//! `Analysis.h` / `PassManager.h` pieces needed by llvmkit's first
//! function and module analyses.

use std::any::{Any, TypeId, type_name};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::dominator_tree::{DominatorTree, DominatorTreeAnalysis};
use crate::function::FunctionValue;
use crate::marker::Dyn;
use crate::module::{Module, ModuleId};
use crate::pass_instrumentation::PassInstrumentationCallbacks;
use crate::value::ValueId;
use crate::{IrError, IrResult};

/// Marker set for all module analyses. Mirrors `AllAnalysesOn<Module>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AllAnalysesOnModule;

/// Marker set for all function analyses. Mirrors `AllAnalysesOn<Function>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AllAnalysesOnFunction;

/// Marker set for analyses that only depend on function CFG shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CFGAnalyses;

/// Set of analyses preserved by a pass. Analysis and set identities use
/// `TypeId`, not pointer addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreservedAnalyses {
    all: bool,
    preserved: HashSet<TypeId>,
    preserved_sets: HashSet<TypeId>,
    abandoned: HashSet<TypeId>,
}

impl Default for PreservedAnalyses {
    fn default() -> Self {
        Self::none()
    }
}

impl PreservedAnalyses {
    /// Preserve no analyses.
    pub fn none() -> Self {
        Self {
            all: false,
            preserved: HashSet::new(),
            preserved_sets: HashSet::new(),
            abandoned: HashSet::new(),
        }
    }

    /// Preserve every analysis unless later abandoned.
    pub fn all() -> Self {
        Self {
            all: true,
            preserved: HashSet::new(),
            preserved_sets: HashSet::new(),
            abandoned: HashSet::new(),
        }
    }

    /// Mark one concrete analysis as preserved.
    pub fn preserve<A: 'static>(&mut self) -> &mut Self {
        let id = TypeId::of::<A>();
        self.abandoned.remove(&id);
        if !self.all {
            self.preserved.insert(id);
        }
        self
    }

    /// Mark one abstract analysis set as preserved.
    pub fn preserve_set<S: 'static>(&mut self) -> &mut Self {
        if !self.all {
            self.preserved_sets.insert(TypeId::of::<S>());
        }
        self
    }

    /// Mark one concrete analysis as explicitly not preserved.
    pub fn abandon<A: 'static>(&mut self) -> &mut Self {
        let id = TypeId::of::<A>();
        self.preserved.remove(&id);
        self.abandoned.insert(id);
        self
    }

    /// Intersect with another preserved set.
    pub fn intersect(&mut self, other: PreservedAnalyses) {
        if other.all {
            return;
        }
        if self.all {
            *self = other;
            return;
        }
        for id in other.abandoned {
            self.preserved.remove(&id);
            self.abandoned.insert(id);
        }
        self.preserved.retain(|id| other.preserved.contains(id));
        self.preserved_sets
            .retain(|id| other.preserved_sets.contains(id));
    }

    /// Build a checker for `A`.
    pub fn checker<A: 'static>(&self) -> PreservedAnalysisChecker<'_> {
        PreservedAnalysisChecker {
            pa: self,
            analysis: TypeId::of::<A>(),
        }
    }
}

/// Query object equivalent to LLVM's `PreservedAnalyses::getChecker`.
#[derive(Debug, Clone, Copy)]
pub struct PreservedAnalysisChecker<'a> {
    pa: &'a PreservedAnalyses,
    analysis: TypeId,
}

impl PreservedAnalysisChecker<'_> {
    /// Whether the concrete analysis is preserved.
    pub fn preserved(self) -> bool {
        !self.pa.abandoned.contains(&self.analysis)
            && (self.pa.all || self.pa.preserved.contains(&self.analysis))
    }

    /// Whether an abstract analysis set is preserved.
    pub fn preserved_set<S: 'static>(self) -> bool {
        self.pa.all || self.pa.preserved_sets.contains(&TypeId::of::<S>())
    }
}

/// A module analysis pass.
pub trait ModuleAnalysis<'ctx>: 'static {
    type Result: ModuleAnalysisResult<'ctx> + 'static;

    fn run(
        &self,
        module: &'ctx Module<'ctx>,
        am: &mut ModuleAnalysisManager<'ctx>,
    ) -> IrResult<Self::Result>;
}

/// Cached module-analysis result.
pub trait ModuleAnalysisResult<'ctx>: 'static {
    /// Return `true` when this result should be invalidated.
    fn invalidate(&mut self, _module: &'ctx Module<'ctx>, _pa: &PreservedAnalyses) -> bool {
        true
    }
}

/// A function analysis pass.
pub trait FunctionAnalysis<'ctx>: 'static {
    type Result: FunctionAnalysisResult<'ctx> + 'static;

    fn run(
        &self,
        function: FunctionValue<'ctx, Dyn>,
        am: &mut FunctionAnalysisManager<'ctx>,
    ) -> IrResult<Self::Result>;
}

/// Cached function-analysis result.
pub trait FunctionAnalysisResult<'ctx>: 'static {
    /// Return `true` when this result should be invalidated.
    fn invalidate(&mut self, _function: FunctionValue<'ctx, Dyn>, _pa: &PreservedAnalyses) -> bool {
        true
    }
}

type FunctionRunner<'ctx> = Rc<
    dyn Fn(
            FunctionValue<'ctx, Dyn>,
            &mut FunctionAnalysisManager<'ctx>,
        ) -> IrResult<CachedFunctionResult<'ctx>>
        + 'ctx,
>;

type ModuleRunner<'ctx> = Rc<
    dyn Fn(
            &'ctx Module<'ctx>,
            &mut ModuleAnalysisManager<'ctx>,
        ) -> IrResult<CachedModuleResult<'ctx>>
        + 'ctx,
>;

struct CachedFunctionResult<'ctx> {
    result: Box<dyn Any>,
    invalidate: fn(&mut dyn Any, FunctionValue<'ctx, Dyn>, &PreservedAnalyses) -> bool,
}

struct CachedModuleResult<'ctx> {
    result: Box<dyn Any>,
    invalidate: fn(&mut dyn Any, &'ctx Module<'ctx>, &PreservedAnalyses) -> bool,
}

/// Caches function analyses by `(analysis type, function id)`.
pub struct FunctionAnalysisManager<'ctx> {
    analyses: HashMap<TypeId, FunctionRunner<'ctx>>,
    results: HashMap<(TypeId, ValueId), CachedFunctionResult<'ctx>>,
    instrumentation: Option<PassInstrumentationCallbacks>,
}

impl<'ctx> FunctionAnalysisManager<'ctx> {
    pub fn new() -> Self {
        Self {
            analyses: HashMap::new(),
            results: HashMap::new(),
            instrumentation: None,
        }
    }

    pub fn set_instrumentation(&mut self, callbacks: PassInstrumentationCallbacks) {
        self.instrumentation = Some(callbacks);
    }

    pub fn register_pass<A>(&mut self, analysis: A)
    where
        A: FunctionAnalysis<'ctx>,
    {
        let id = TypeId::of::<A>();
        let runner: FunctionRunner<'ctx> = Rc::new(move |function, am| {
            let result = analysis.run(function, am)?;
            Ok(CachedFunctionResult {
                result: Box::new(result),
                invalidate: invalidate_function_result::<A>,
            })
        });
        self.analyses.insert(id, runner);
    }

    pub fn get_result<A>(&mut self, function: FunctionValue<'ctx, Dyn>) -> IrResult<&A::Result>
    where
        A: FunctionAnalysis<'ctx>,
    {
        let key = (TypeId::of::<A>(), function.as_value().id);
        if !self.results.contains_key(&key) {
            let Some(runner) = self.analyses.get(&key.0).cloned() else {
                return Err(IrError::AnalysisNotRegistered {
                    name: type_name::<A>(),
                });
            };
            if let Some(callbacks) = &self.instrumentation {
                callbacks.run_before_analysis(type_name::<A>());
            }
            let result = runner(function, self)?;
            self.results.insert(key, result);
            if let Some(callbacks) = &self.instrumentation {
                callbacks.run_after_analysis(type_name::<A>());
            }
        }
        self.get_cached_result::<A>(function)
            .ok_or(IrError::AnalysisNotRegistered {
                name: type_name::<A>(),
            })
    }

    pub fn get_cached_result<A>(&self, function: FunctionValue<'ctx, Dyn>) -> Option<&A::Result>
    where
        A: FunctionAnalysis<'ctx>,
    {
        self.results
            .get(&(TypeId::of::<A>(), function.as_value().id))?
            .result
            .downcast_ref::<A::Result>()
    }

    pub fn invalidate(&mut self, function: FunctionValue<'ctx, Dyn>, pa: &PreservedAnalyses) {
        let function_id = function.as_value().id;
        let mut dead = Vec::new();
        for (key, cached) in &mut self.results {
            if key.1 == function_id && (cached.invalidate)(&mut *cached.result, function, pa) {
                dead.push(*key);
            }
        }
        for key in dead {
            self.results.remove(&key);
        }
    }

    pub fn clear(&mut self) {
        self.results.clear();
    }

    pub fn clear_analysis<A>(&mut self, function: FunctionValue<'ctx, Dyn>)
    where
        A: FunctionAnalysis<'ctx>,
    {
        self.results
            .remove(&(TypeId::of::<A>(), function.as_value().id));
    }
}

impl Default for FunctionAnalysisManager<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// Caches module analyses by `(analysis type, module id)`.
pub struct ModuleAnalysisManager<'ctx> {
    analyses: HashMap<TypeId, ModuleRunner<'ctx>>,
    results: HashMap<(TypeId, ModuleId), CachedModuleResult<'ctx>>,
    instrumentation: Option<PassInstrumentationCallbacks>,
}

impl<'ctx> ModuleAnalysisManager<'ctx> {
    pub fn new() -> Self {
        Self {
            analyses: HashMap::new(),
            results: HashMap::new(),
            instrumentation: None,
        }
    }

    pub fn set_instrumentation(&mut self, callbacks: PassInstrumentationCallbacks) {
        self.instrumentation = Some(callbacks);
    }

    pub fn register_pass<A>(&mut self, analysis: A)
    where
        A: ModuleAnalysis<'ctx>,
    {
        let id = TypeId::of::<A>();
        let runner: ModuleRunner<'ctx> = Rc::new(move |module, am| {
            let result = analysis.run(module, am)?;
            Ok(CachedModuleResult {
                result: Box::new(result),
                invalidate: invalidate_module_result::<A>,
            })
        });
        self.analyses.insert(id, runner);
    }

    pub fn get_result<A>(&mut self, module: &'ctx Module<'ctx>) -> IrResult<&A::Result>
    where
        A: ModuleAnalysis<'ctx>,
    {
        let key = (TypeId::of::<A>(), module.id());
        if !self.results.contains_key(&key) {
            let Some(runner) = self.analyses.get(&key.0).cloned() else {
                return Err(IrError::AnalysisNotRegistered {
                    name: type_name::<A>(),
                });
            };
            if let Some(callbacks) = &self.instrumentation {
                callbacks.run_before_analysis(type_name::<A>());
            }
            let result = runner(module, self)?;
            self.results.insert(key, result);
            if let Some(callbacks) = &self.instrumentation {
                callbacks.run_after_analysis(type_name::<A>());
            }
        }
        self.get_cached_result::<A>(module)
            .ok_or(IrError::AnalysisNotRegistered {
                name: type_name::<A>(),
            })
    }

    pub fn get_cached_result<A>(&self, module: &'ctx Module<'ctx>) -> Option<&A::Result>
    where
        A: ModuleAnalysis<'ctx>,
    {
        self.results
            .get(&(TypeId::of::<A>(), module.id()))?
            .result
            .downcast_ref::<A::Result>()
    }

    pub fn invalidate(&mut self, module: &'ctx Module<'ctx>, pa: &PreservedAnalyses) {
        let module_id = module.id();
        let mut dead = Vec::new();
        for (key, cached) in &mut self.results {
            if key.1 == module_id && (cached.invalidate)(&mut *cached.result, module, pa) {
                dead.push(*key);
            }
        }
        for key in dead {
            self.results.remove(&key);
        }
    }

    pub fn clear(&mut self) {
        self.results.clear();
    }

    pub fn clear_analysis<A>(&mut self, module: &'ctx Module<'ctx>)
    where
        A: ModuleAnalysis<'ctx>,
    {
        self.results.remove(&(TypeId::of::<A>(), module.id()));
    }
}

impl Default for ModuleAnalysisManager<'_> {
    fn default() -> Self {
        Self::new()
    }
}

fn invalidate_function_result<'ctx, A>(
    result: &mut dyn Any,
    function: FunctionValue<'ctx, Dyn>,
    pa: &PreservedAnalyses,
) -> bool
where
    A: FunctionAnalysis<'ctx>,
{
    let Some(result) = result.downcast_mut::<A::Result>() else {
        return true;
    };
    result.invalidate(function, pa)
}

fn invalidate_module_result<'ctx, A>(
    result: &mut dyn Any,
    module: &'ctx Module<'ctx>,
    pa: &PreservedAnalyses,
) -> bool
where
    A: ModuleAnalysis<'ctx>,
{
    let Some(result) = result.downcast_mut::<A::Result>() else {
        return true;
    };
    result.invalidate(module, pa)
}

impl<'ctx> FunctionAnalysis<'ctx> for DominatorTreeAnalysis {
    type Result = DominatorTree;

    fn run(
        &self,
        function: FunctionValue<'ctx, Dyn>,
        _am: &mut FunctionAnalysisManager<'ctx>,
    ) -> IrResult<Self::Result> {
        Ok(DominatorTree::new(function))
    }
}

impl<'ctx> FunctionAnalysisResult<'ctx> for DominatorTree {
    fn invalidate(&mut self, _function: FunctionValue<'ctx, Dyn>, pa: &PreservedAnalyses) -> bool {
        let checker = pa.checker::<DominatorTreeAnalysis>();
        !(checker.preserved()
            || checker.preserved_set::<AllAnalysesOnFunction>()
            || checker.preserved_set::<CFGAnalyses>())
    }
}
