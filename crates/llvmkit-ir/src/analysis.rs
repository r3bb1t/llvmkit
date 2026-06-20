//! Minimal LLVM-new-PM-style analysis substrate. Mirrors the
//! `Analysis.h` / `PassManager.h` pieces needed by llvmkit's first
//! function and module analyses.

use std::any::{Any, TypeId, type_name};
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::rc::Rc;

use crate::dominator_tree::{DominatorTree, DominatorTreeAnalysis};
use crate::module::{Brand, ModuleBrand, ModuleId, ModuleView};
use crate::pass_context::FunctionView;
use crate::pass_instrumentation::PassInstrumentationCallbacks;
use crate::value::ValueId;
use crate::{IrError, IrResult};

/// Explicit analysis identity used when no Rust type exists for a ported
/// upstream `AnalysisKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnalysisKeyId(u64);

impl AnalysisKeyId {
    #[inline]
    pub const fn new(id: u64) -> Self {
        Self(id)
    }
}

/// Explicit analysis-set identity used when no Rust type exists for a ported
/// upstream `AnalysisSetKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnalysisSetKeyId(u64);

impl AnalysisSetKeyId {
    #[inline]
    pub const fn new(id: u64) -> Self {
        Self(id)
    }
}

/// Marker set for all module analyses. Mirrors `AllAnalysesOn<Module>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AllAnalysesOnModule;

/// Marker set for all function analyses. Mirrors `AllAnalysesOn<Function>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AllAnalysesOnFunction;

/// Marker set for analyses that only depend on function CFG shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CFGAnalyses;

/// Marker analysis modelling LLVM's `FunctionAnalysisManagerModuleProxy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FunctionAnalysisManagerModuleProxy;

/// Set of analyses preserved by a pass. Analysis and set identities use stable
/// typed keys, not pointer addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreservedAnalyses {
    all: bool,
    preserved: HashSet<TypeId>,
    preserved_sets: HashSet<TypeId>,
    abandoned: HashSet<TypeId>,
    preserved_keys: HashSet<AnalysisKeyId>,
    preserved_set_keys: HashSet<AnalysisSetKeyId>,
    abandoned_keys: HashSet<AnalysisKeyId>,
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
            preserved_keys: HashSet::new(),
            preserved_set_keys: HashSet::new(),
            abandoned_keys: HashSet::new(),
        }
    }

    /// Preserve every analysis unless later abandoned.
    pub fn all() -> Self {
        Self {
            all: true,
            preserved: HashSet::new(),
            preserved_sets: HashSet::new(),
            abandoned: HashSet::new(),
            preserved_keys: HashSet::new(),
            preserved_set_keys: HashSet::new(),
            abandoned_keys: HashSet::new(),
        }
    }

    /// Preserve every analysis in one typed set.
    pub fn all_in_set<S: 'static>() -> Self {
        let mut pa = Self::none();
        pa.preserve_set::<S>();
        pa
    }

    /// Preserve every analysis in one explicit-key set.
    pub fn all_in_set_key(set: AnalysisSetKeyId) -> Self {
        let mut pa = Self::none();
        pa.preserve_set_key(set);
        pa
    }

    /// Whether every analysis is preserved and no key has been abandoned.
    pub fn are_all_preserved(&self) -> bool {
        self.all && self.abandoned.is_empty() && self.abandoned_keys.is_empty()
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

    /// Mark one explicit analysis key as preserved.
    pub fn preserve_key(&mut self, key: AnalysisKeyId) -> &mut Self {
        self.abandoned_keys.remove(&key);
        if !self.all {
            self.preserved_keys.insert(key);
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

    /// Mark one explicit analysis set key as preserved.
    pub fn preserve_set_key(&mut self, set: AnalysisSetKeyId) -> &mut Self {
        if !self.all {
            self.preserved_set_keys.insert(set);
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

    /// Mark one explicit analysis key as not preserved.
    pub fn abandon_key(&mut self, key: AnalysisKeyId) -> &mut Self {
        self.preserved_keys.remove(&key);
        self.abandoned_keys.insert(key);
        self
    }

    /// Whether every analysis in a typed set is preserved.
    pub fn all_analyses_in_set_preserved<S: 'static>(&self) -> bool {
        self.abandoned.is_empty()
            && self.abandoned_keys.is_empty()
            && (self.all || self.preserved_sets.contains(&TypeId::of::<S>()))
    }

    /// Whether every analysis in an explicit-key set is preserved.
    pub fn all_analyses_in_set_key_preserved(&self, set: AnalysisSetKeyId) -> bool {
        self.abandoned.is_empty()
            && self.abandoned_keys.is_empty()
            && (self.all || self.preserved_set_keys.contains(&set))
    }

    /// Intersect with another preserved set.
    pub fn intersect(&mut self, other: PreservedAnalyses) {
        if self.all && other.all {
            self.abandoned.extend(other.abandoned);
            self.abandoned_keys.extend(other.abandoned_keys);
            return;
        }

        if self.all {
            let abandoned = self.abandoned.clone();
            let abandoned_keys = self.abandoned_keys.clone();
            *self = other;
            self.abandoned.extend(abandoned);
            self.abandoned_keys.extend(abandoned_keys);
            self.drop_abandoned();
            return;
        }

        if !other.all {
            self.preserved.retain(|id| other.preserved.contains(id));
            self.preserved_sets
                .retain(|id| other.preserved_sets.contains(id));
            self.preserved_keys
                .retain(|key| other.preserved_keys.contains(key));
            self.preserved_set_keys
                .retain(|set| other.preserved_set_keys.contains(set));
        }

        self.abandoned.extend(other.abandoned);
        self.abandoned_keys.extend(other.abandoned_keys);
        self.drop_abandoned();
    }

    /// Build a checker for `A`.
    pub fn checker<A: 'static>(&self) -> PreservedAnalysisChecker<'_> {
        PreservedAnalysisChecker {
            pa: self,
            analysis: TypeId::of::<A>(),
            key: None,
        }
    }

    /// Build a checker for an explicit analysis key.
    pub fn checker_for_key(&self, key: AnalysisKeyId) -> PreservedAnalysisChecker<'_> {
        PreservedAnalysisChecker {
            pa: self,
            analysis: TypeId::of::<()>(),
            key: Some(key),
        }
    }

    fn drop_abandoned(&mut self) {
        for id in &self.abandoned {
            self.preserved.remove(id);
        }
        for key in &self.abandoned_keys {
            self.preserved_keys.remove(key);
        }
    }
}

/// Query object equivalent to LLVM's `PreservedAnalyses::getChecker`.
#[derive(Debug, Clone, Copy)]
pub struct PreservedAnalysisChecker<'a> {
    pa: &'a PreservedAnalyses,
    analysis: TypeId,
    key: Option<AnalysisKeyId>,
}

impl PreservedAnalysisChecker<'_> {
    /// Whether the concrete analysis is preserved.
    pub fn preserved(self) -> bool {
        match self.key {
            Some(key) => {
                !self.pa.abandoned_keys.contains(&key)
                    && (self.pa.all || self.pa.preserved_keys.contains(&key))
            }
            None => {
                !self.pa.abandoned.contains(&self.analysis)
                    && (self.pa.all || self.pa.preserved.contains(&self.analysis))
            }
        }
    }

    /// Whether a typed analysis set is preserved for this analysis.
    pub fn preserved_set<S: 'static>(self) -> bool {
        if self.key.is_some() {
            return false;
        }
        !self.pa.abandoned.contains(&self.analysis)
            && (self.pa.all || self.pa.preserved_sets.contains(&TypeId::of::<S>()))
    }

    /// Whether an explicit-key analysis set is preserved for this key.
    pub fn preserved_set_key(self, set: AnalysisSetKeyId) -> bool {
        let Some(key) = self.key else {
            return false;
        };
        !self.pa.abandoned_keys.contains(&key)
            && (self.pa.all || self.pa.preserved_set_keys.contains(&set))
    }

    /// Whether a stateless analysis result could be reused.
    pub fn preserved_when_stateless(self) -> bool {
        match self.key {
            Some(key) => !self.pa.abandoned_keys.contains(&key),
            None => !self.pa.abandoned.contains(&self.analysis),
        }
    }
}

/// A module analysis pass.
pub trait ModuleAnalysis<'ctx, B: ModuleBrand = Brand<'ctx>>: 'static {
    type Result: ModuleAnalysisResult<'ctx, B> + 'static;

    fn run(
        &self,
        module: ModuleView<'ctx, B>,
        am: &mut ModuleAnalysisManager<'ctx, B>,
    ) -> IrResult<Self::Result>;
}

/// Cached module-analysis result.
pub trait ModuleAnalysisResult<'ctx, B: ModuleBrand = Brand<'ctx>>: 'static {
    /// Return `true` when this result should be invalidated.
    fn invalidate(
        &mut self,
        _module: ModuleView<'ctx, B>,
        _pa: &PreservedAnalyses,
        _inv: &mut ModuleAnalysisInvalidator<'_, 'ctx, B>,
    ) -> IrResult<bool> {
        Ok(true)
    }
}

/// A function analysis pass.
pub trait FunctionAnalysis<'ctx, B: ModuleBrand = Brand<'ctx>>: 'static {
    type Result: FunctionAnalysisResult<'ctx, B> + 'static;

    fn run(
        &self,
        function: FunctionView<'ctx, B>,
        am: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Self::Result>;
}

/// Cached function-analysis result.
pub trait FunctionAnalysisResult<'ctx, B: ModuleBrand = Brand<'ctx>>: 'static {
    /// Return `true` when this result should be invalidated.
    fn invalidate(
        &mut self,
        _function: FunctionView<'ctx, B>,
        _pa: &PreservedAnalyses,
        _inv: &mut FunctionAnalysisInvalidator<'_, 'ctx, B>,
    ) -> IrResult<bool> {
        Ok(true)
    }
}

type FunctionRunner<'ctx, B> = Rc<
    dyn Fn(
            FunctionView<'ctx, B>,
            &mut FunctionAnalysisManager<'ctx, B>,
        ) -> IrResult<CachedFunctionResult<'ctx, B>>
        + 'ctx,
>;

type ModuleRunner<'ctx, B> = Rc<
    dyn Fn(
            ModuleView<'ctx, B>,
            &mut ModuleAnalysisManager<'ctx, B>,
        ) -> IrResult<CachedModuleResult<'ctx, B>>
        + 'ctx,
>;

type FunctionInvalidator<'ctx, B> = fn(
    &mut dyn Any,
    FunctionView<'ctx, B>,
    &PreservedAnalyses,
    &FunctionAnalysisSnapshot,
) -> IrResult<bool>;

type ModuleInvalidator<'ctx, B> = fn(
    &mut dyn Any,
    ModuleView<'ctx, B>,
    &PreservedAnalyses,
    &ModuleAnalysisSnapshot,
) -> IrResult<bool>;

struct CachedFunctionResult<'ctx, B: ModuleBrand> {
    result: Box<dyn Any>,
    invalidate: FunctionInvalidator<'ctx, B>,
}

struct CachedModuleResult<'ctx, B: ModuleBrand> {
    result: Box<dyn Any>,
    invalidate: ModuleInvalidator<'ctx, B>,
}

#[derive(Clone)]
struct FunctionAnalysisSnapshot {
    cached: HashSet<(ModuleId, TypeId, ValueId)>,
}

#[derive(Clone)]
struct ModuleAnalysisSnapshot {
    cached: HashSet<(TypeId, ModuleId)>,
}

/// Invalidator passed to function-analysis results.
pub struct FunctionAnalysisInvalidator<'a, 'ctx, B: ModuleBrand = Brand<'ctx>> {
    function: FunctionView<'ctx, B>,
    pa: &'a PreservedAnalyses,
    snapshot: &'a FunctionAnalysisSnapshot,
}

impl<'a, 'ctx, B: ModuleBrand> FunctionAnalysisInvalidator<'a, 'ctx, B> {
    pub fn invalidate<A>(&mut self) -> IrResult<bool>
    where
        A: FunctionAnalysis<'ctx, B>,
    {
        let key = function_key::<A, B>(self.function);
        if !self.snapshot.cached.contains(&key) {
            return Err(IrError::AnalysisNotCached {
                name: type_name::<A>(),
            });
        }
        let checker = self.pa.checker::<A>();
        Ok(!(checker.preserved() || checker.preserved_set::<AllAnalysesOnFunction>()))
    }
}

/// Invalidator passed to module-analysis results.
pub struct ModuleAnalysisInvalidator<'a, 'ctx, B: ModuleBrand = Brand<'ctx>> {
    module: ModuleView<'ctx, B>,
    pa: &'a PreservedAnalyses,
    snapshot: &'a ModuleAnalysisSnapshot,
}

impl<'a, 'ctx, B: ModuleBrand + 'ctx> ModuleAnalysisInvalidator<'a, 'ctx, B> {
    pub fn invalidate<A>(&mut self) -> IrResult<bool>
    where
        A: ModuleAnalysis<'ctx, B>,
    {
        let key = module_key::<A, B>(self.module);
        if !self.snapshot.cached.contains(&key) {
            return Err(IrError::AnalysisNotCached {
                name: type_name::<A>(),
            });
        }
        let checker = self.pa.checker::<A>();
        Ok(!(checker.preserved() || checker.preserved_set::<AllAnalysesOnModule>()))
    }
}

/// Caches function analyses by `(module id, analysis type, function id)`.
pub struct FunctionAnalysisManager<'ctx, B: ModuleBrand = Brand<'ctx>> {
    analyses: HashMap<TypeId, FunctionRunner<'ctx, B>>,
    results: HashMap<(ModuleId, TypeId, ValueId), CachedFunctionResult<'ctx, B>>,
    instrumentation: Option<PassInstrumentationCallbacks>,
    _brand: PhantomData<fn(B) -> B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionAnalysisManager<'ctx, B> {
    pub fn new() -> Self {
        Self {
            analyses: HashMap::new(),
            results: HashMap::new(),
            instrumentation: None,
            _brand: PhantomData,
        }
    }

    pub fn set_instrumentation(&mut self, callbacks: PassInstrumentationCallbacks) {
        self.instrumentation = Some(callbacks);
    }

    pub fn register_pass<A>(&mut self, analysis: A)
    where
        A: FunctionAnalysis<'ctx, B>,
    {
        let id = TypeId::of::<A>();
        let runner: FunctionRunner<'ctx, B> = Rc::new(move |function, am| {
            let result = analysis.run(function, am)?;
            Ok(CachedFunctionResult {
                result: Box::new(result),
                invalidate: invalidate_function_result::<B, A>,
            })
        });
        self.analyses.insert(id, runner);
    }

    pub fn get_result<A>(
        &mut self,
        function: impl Into<FunctionView<'ctx, B>>,
    ) -> IrResult<&A::Result>
    where
        A: FunctionAnalysis<'ctx, B>,
    {
        let function = function.into();
        let key = function_key::<A, B>(function);
        if !self.results.contains_key(&key) {
            let Some(runner) = self.analyses.get(&key.1).cloned() else {
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
            .ok_or(IrError::AnalysisNotCached {
                name: type_name::<A>(),
            })
    }

    pub fn get_cached_result<A>(
        &self,
        function: impl Into<FunctionView<'ctx, B>>,
    ) -> Option<&A::Result>
    where
        A: FunctionAnalysis<'ctx, B>,
    {
        let function = function.into();
        self.results
            .get(&function_key::<A, B>(function))?
            .result
            .downcast_ref::<A::Result>()
    }

    pub fn invalidate(
        &mut self,
        function: impl Into<FunctionView<'ctx, B>>,
        pa: &PreservedAnalyses,
    ) -> IrResult<()> {
        let function = function.into();
        let function_handle = function.as_function();
        let module_id = function_handle.as_value().module().id();
        let function_id = function_handle.as_value().id;
        let snapshot = FunctionAnalysisSnapshot {
            cached: self.results.keys().copied().collect(),
        };
        let mut dead = Vec::new();
        for (key, cached) in &mut self.results {
            if key.0 == module_id
                && key.2 == function_id
                && (cached.invalidate)(&mut *cached.result, function, pa, &snapshot)?
            {
                dead.push(*key);
            }
        }
        for key in dead {
            self.results.remove(&key);
        }
        Ok(())
    }

    pub fn invalidate_module(
        &mut self,
        module: ModuleView<'ctx, B>,
        pa: &PreservedAnalyses,
    ) -> IrResult<()> {
        let proxy = pa.checker::<FunctionAnalysisManagerModuleProxy>();
        if pa.are_all_preserved()
            || (proxy.preserved() && pa.all_analyses_in_set_preserved::<AllAnalysesOnFunction>())
        {
            return Ok(());
        }
        let module_id = module.id();
        self.results.retain(|key, _| key.0 != module_id);
        Ok(())
    }

    pub fn clear(&mut self) {
        self.results.clear();
    }

    pub fn clear_analysis<A>(&mut self, function: impl Into<FunctionView<'ctx, B>>)
    where
        A: FunctionAnalysis<'ctx, B>,
    {
        let function = function.into();
        self.results.remove(&function_key::<A, B>(function));
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> Default for FunctionAnalysisManager<'ctx, B> {
    fn default() -> Self {
        Self::new()
    }
}

/// Caches module analyses by `(analysis type, module id)`.
pub struct ModuleAnalysisManager<'ctx, B: ModuleBrand = Brand<'ctx>> {
    analyses: HashMap<TypeId, ModuleRunner<'ctx, B>>,
    results: HashMap<(TypeId, ModuleId), CachedModuleResult<'ctx, B>>,
    instrumentation: Option<PassInstrumentationCallbacks>,
    _brand: PhantomData<fn(B) -> B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> ModuleAnalysisManager<'ctx, B> {
    pub fn new() -> Self {
        Self {
            analyses: HashMap::new(),
            results: HashMap::new(),
            instrumentation: None,
            _brand: PhantomData,
        }
    }

    pub fn set_instrumentation(&mut self, callbacks: PassInstrumentationCallbacks) {
        self.instrumentation = Some(callbacks);
    }

    pub fn register_pass<A>(&mut self, analysis: A)
    where
        A: ModuleAnalysis<'ctx, B>,
    {
        let id = TypeId::of::<A>();
        let runner: ModuleRunner<'ctx, B> = Rc::new(move |module, am| {
            let result = analysis.run(module, am)?;
            Ok(CachedModuleResult {
                result: Box::new(result),
                invalidate: invalidate_module_result::<B, A>,
            })
        });
        self.analyses.insert(id, runner);
    }

    pub fn get_result<A>(
        &mut self,
        module: &crate::module::Module<'ctx, B, crate::module::Verified>,
    ) -> IrResult<&A::Result>
    where
        A: ModuleAnalysis<'ctx, B>,
    {
        let module_view = module.as_view();
        let key = module_key::<A, B>(module_view);
        if !self.results.contains_key(&key) {
            let Some(runner) = self.analyses.get(&key.0).cloned() else {
                return Err(IrError::AnalysisNotRegistered {
                    name: type_name::<A>(),
                });
            };
            if let Some(callbacks) = &self.instrumentation {
                callbacks.run_before_analysis(type_name::<A>());
            }
            let result = runner(module_view, self)?;
            self.results.insert(key, result);
            if let Some(callbacks) = &self.instrumentation {
                callbacks.run_after_analysis(type_name::<A>());
            }
        }
        self.get_cached_result::<A>(module_view)
            .ok_or(IrError::AnalysisNotCached {
                name: type_name::<A>(),
            })
    }

    pub fn get_cached_result<A>(&self, module: impl Into<ModuleView<'ctx, B>>) -> Option<&A::Result>
    where
        A: ModuleAnalysis<'ctx, B>,
    {
        let module = module.into();
        self.results
            .get(&module_key::<A, B>(module))?
            .result
            .downcast_ref::<A::Result>()
    }

    pub fn invalidate(
        &mut self,
        module: impl Into<ModuleView<'ctx, B>>,
        pa: &PreservedAnalyses,
    ) -> IrResult<()> {
        let module = module.into();
        let module_id = module.id();
        let snapshot = ModuleAnalysisSnapshot {
            cached: self.results.keys().copied().collect(),
        };
        let mut dead = Vec::new();
        for (key, cached) in &mut self.results {
            if key.1 == module_id
                && (cached.invalidate)(&mut *cached.result, module, pa, &snapshot)?
            {
                dead.push(*key);
            }
        }
        for key in dead {
            self.results.remove(&key);
        }
        Ok(())
    }

    pub fn clear(&mut self) {
        self.results.clear();
    }

    pub fn clear_analysis<A>(&mut self, module: impl Into<ModuleView<'ctx, B>>)
    where
        A: ModuleAnalysis<'ctx, B>,
    {
        let module = module.into();
        self.results.remove(&module_key::<A, B>(module));
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> Default for ModuleAnalysisManager<'ctx, B> {
    fn default() -> Self {
        Self::new()
    }
}

fn function_key<'ctx, A, B>(function: FunctionView<'ctx, B>) -> (ModuleId, TypeId, ValueId)
where
    A: 'static,
    B: ModuleBrand + 'ctx,
{
    let function = function.as_function();
    (
        function.as_value().module().id(),
        TypeId::of::<A>(),
        function.as_value().id,
    )
}

fn module_key<'ctx, A, B>(module: ModuleView<'ctx, B>) -> (TypeId, ModuleId)
where
    A: 'static,
    B: ModuleBrand + 'ctx,
{
    (TypeId::of::<A>(), module.id())
}

fn invalidate_function_result<'ctx, B, A>(
    result: &mut dyn Any,
    function: FunctionView<'ctx, B>,
    pa: &PreservedAnalyses,
    snapshot: &FunctionAnalysisSnapshot,
) -> IrResult<bool>
where
    B: ModuleBrand + 'ctx,
    A: FunctionAnalysis<'ctx, B>,
{
    let Some(result) = result.downcast_mut::<A::Result>() else {
        return Ok(true);
    };
    let mut invalidator = FunctionAnalysisInvalidator {
        function,
        pa,
        snapshot,
    };
    result.invalidate(function, pa, &mut invalidator)
}

fn invalidate_module_result<'ctx, B, A>(
    result: &mut dyn Any,
    module: ModuleView<'ctx, B>,
    pa: &PreservedAnalyses,
    snapshot: &ModuleAnalysisSnapshot,
) -> IrResult<bool>
where
    B: ModuleBrand + 'ctx,
    A: ModuleAnalysis<'ctx, B>,
{
    let Some(result) = result.downcast_mut::<A::Result>() else {
        return Ok(true);
    };
    let mut invalidator = ModuleAnalysisInvalidator {
        module,
        pa,
        snapshot,
    };
    result.invalidate(module, pa, &mut invalidator)
}

impl<'ctx> FunctionAnalysis<'ctx> for DominatorTreeAnalysis {
    type Result = DominatorTree;

    fn run(
        &self,
        function: FunctionView<'ctx>,
        _am: &mut FunctionAnalysisManager<'ctx>,
    ) -> IrResult<Self::Result> {
        Ok(DominatorTree::new(function.as_function()))
    }
}

impl<'ctx> FunctionAnalysisResult<'ctx> for DominatorTree {
    fn invalidate(
        &mut self,
        _function: FunctionView<'ctx>,
        pa: &PreservedAnalyses,
        _inv: &mut FunctionAnalysisInvalidator<'_, 'ctx>,
    ) -> IrResult<bool> {
        let checker = pa.checker::<DominatorTreeAnalysis>();
        Ok(!(checker.preserved()
            || checker.preserved_set::<AllAnalysesOnFunction>()
            || checker.preserved_set::<CFGAnalyses>()))
    }
}
