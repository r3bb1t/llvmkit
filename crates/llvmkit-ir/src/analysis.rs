//! Minimal LLVM-new-PM-style analysis substrate. Mirrors the
//! `Analysis.h` / `PassManager.h` pieces needed by llvmkit's first
//! function and module analyses.

#![deny(missing_docs)]

use std::any::{Any, TypeId, type_name};
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::rc::Rc;

use crate::cfg_update::CfgUpdate;
use crate::dominator_tree::{DominatorTree, DominatorTreeAnalysis};
use crate::module::{Brand, ModuleBrand, ModuleId, ModuleView};
use crate::pass_context::FunctionView;
use crate::pass_instrumentation::PassInstrumentationCallbacks;
use crate::value::{IsValue, ValueId};
use crate::{IrError, IrResult};

/// Explicit analysis identity used when no Rust type exists for a ported
/// upstream `AnalysisKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnalysisKeyId(u64);

impl AnalysisKeyId {
    /// Wrap a raw 64-bit identifier as an analysis key.
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
    /// Wrap a raw 64-bit identifier as an analysis-set key.
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

    /// Mark one concrete analysis as preserved by its already-resolved
    /// [`TypeId`]. The type-erased twin of [`Self::preserve`], used by the
    /// reshape `done()`-flush ([`FunctionAnalysisManager::flush_cfg_updates`]),
    /// which iterates cached results by key and cannot name each analysis type.
    pub(crate) fn preserve_type_id(&mut self, id: TypeId) -> &mut Self {
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
    /// The cached result value this analysis produces.
    type Result: ModuleAnalysisResult<'ctx, B> + 'static;

    /// Compute the analysis over `module`, using `am` to fetch any analyses it
    /// depends on.
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
    /// The cached result value this analysis produces.
    type Result: FunctionAnalysisResult<'ctx, B> + 'static;

    /// Compute the analysis over `function`, using `am` to fetch any analyses
    /// it depends on.
    fn run(
        &self,
        function: FunctionView<'ctx, B>,
        am: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Self::Result>;
}

/// How a function analysis registers itself for prefetching, so a typed
/// `Requires` list need not bound its members `Default`. A `Default` analysis
/// auto-registers (delegating to
/// [`FunctionAnalysisManager::ensure_registered_default`]); a parameterized /
/// non-`Default` analysis declares its own strategy — typically a no-op that
/// assumes the caller pre-registered an instance via
/// [`FunctionAnalysisManager::register_pass`].
///
/// Every analysis usable in a typed `Requires` list must implement this. There
/// is deliberately no blanket `impl<A: Default>`: a blanket plus a manual impl
/// for a non-`Default` analysis would overlap under coherence (Rust does no
/// negative reasoning over `Default`), which is exactly the case this trait
/// exists to support. The explicit one-line impls are the cost of dropping the
/// `Default` straitjacket — and they double as the seam where a CFG analysis can
/// opt into incremental preservation (see `register_cfg_pass`).
///
/// No upstream analog: LLVM registers analyses by runtime
/// `AnalysisManager::registerPass` calls with no compile-time `Requires` list.
pub trait PrefetchableAnalysis<'ctx, B: ModuleBrand = Brand<'ctx>>:
    FunctionAnalysis<'ctx, B>
{
    /// Ensure this analysis is registered in `fam`, so a following `get_result`
    /// cannot fail with [`IrError::AnalysisNotRegistered`].
    fn ensure_registered(fam: &mut FunctionAnalysisManager<'ctx, B>);
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

/// What a cached CFG-shaped analysis result did with a batch of recorded
/// [`CfgUpdate`]s. Returned by [`CfgIncremental::apply_updates`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairOutcome {
    /// The result absorbed every update and is now consistent with the edited
    /// CFG — the framework may keep it and mark it preserved.
    Repaired,
    /// The result declined incremental repair; the framework must recompute or
    /// evict it. This is the degenerate default — exactly today's behavior, in
    /// which every mutating rung's floor evicts CFG analyses wholesale.
    PreferRecompute,
}

/// A cached analysis result that can *attempt* to repair itself in place after a
/// batch of CFG edits, instead of being evicted wholesale. This is the
/// framework-witnessed half of Package 4's preservation story: the reshape
/// mutator records [`CfgUpdate`]s as it edits (`cfg_update.rs`), and the driver
/// offers them here — an analysis is only ever marked preserved because the
/// framework *watched* it return [`RepairOutcome::Repaired`], never because an
/// author claimed preservation.
///
/// Implementing this hook is entirely optional: an analysis that does not (or
/// returns [`RepairOutcome::PreferRecompute`]) simply falls back to the existing
/// floor eviction. The update vocabulary is deliberately CFG-shaped; value-level
/// analyses are out of scope (their mutating floor already evicts them).
///
/// No upstream analog in this shape: LLVM hand-feeds `DomTreeUpdater` its edits
/// and trusts the author to keep them complete and ordered; here the edits are
/// framework-recorded and the analysis only ever *reacts* to them.
///
/// [`Sized`] because [`Self::recompute`] returns `Self`: this is only ever
/// implemented on concrete analysis-result types.
pub trait CfgIncremental<'ctx, B: ModuleBrand = Brand<'ctx>>: Sized {
    /// Fold the recorded `updates` (in the order they were performed over
    /// `function`) into this cached result. Return [`RepairOutcome::Repaired`]
    /// only if the result is now fully consistent with the edited CFG;
    /// otherwise return [`RepairOutcome::PreferRecompute`] and the framework
    /// recomputes (via [`Self::recompute`]) or evicts.
    fn apply_updates(
        &mut self,
        updates: &[CfgUpdate],
        function: FunctionView<'ctx, B>,
    ) -> RepairOutcome;

    /// Recompute this analysis from scratch over `function`'s current CFG. The
    /// framework calls this whenever [`Self::apply_updates`] returns
    /// [`RepairOutcome::PreferRecompute`], so a mid-pass read of a CFG analysis
    /// after a reshape edit still yields a *correct* result rather than a stale
    /// cached one. Must equal a fresh construction of the analysis.
    fn recompute(function: FunctionView<'ctx, B>) -> Self;
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

/// Type-erased CFG-incremental repair hook stored on a cached result whose
/// analysis opted in (its `A::Result: CfgIncremental`, registered via
/// [`FunctionAnalysisManager::ensure_cfg_registered_default`]). The driver calls
/// it at the reshape `done()`-flush to offer the recorded edits;
/// [`RepairOutcome::Repaired`] means the result updated itself in place and may
/// be kept preserved.
type CfgApplyFn<'ctx, B> = fn(&mut dyn Any, &[CfgUpdate], FunctionView<'ctx, B>) -> RepairOutcome;

struct CachedFunctionResult<'ctx, B: ModuleBrand> {
    result: Box<dyn Any>,
    invalidate: FunctionInvalidator<'ctx, B>,
    /// `Some` iff this analysis's result implements [`CfgIncremental`]; the
    /// framework-witnessed preservation hook (`None` for value analyses).
    cfg_apply: Option<CfgApplyFn<'ctx, B>>,
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
    /// Report whether analysis `A`'s result for this function is being
    /// invalidated: `true` unless `A` (or the `AllAnalysesOnFunction` set) is
    /// preserved. Errors with [`IrError::AnalysisNotCached`] if `A` was not
    /// cached when invalidation began.
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
    /// Report whether analysis `A`'s result for this module is being
    /// invalidated: `true` unless `A` (or the `AllAnalysesOnModule` set) is
    /// preserved. Errors with [`IrError::AnalysisNotCached`] if `A` was not
    /// cached when invalidation began.
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
    /// Create an empty manager: no analyses registered and no cached results.
    pub fn new() -> Self {
        Self {
            analyses: HashMap::new(),
            results: HashMap::new(),
            instrumentation: None,
            _brand: PhantomData,
        }
    }

    /// Attach the instrumentation callbacks fired before and after each
    /// analysis run.
    pub fn set_instrumentation(&mut self, callbacks: PassInstrumentationCallbacks) {
        self.instrumentation = Some(callbacks);
    }

    /// Register a function-analysis pass instance, keyed by its type, so its
    /// result can be computed on demand by [`Self::get_result`].
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
                cfg_apply: None,
            })
        });
        self.analyses.insert(id, runner);
    }

    /// Register a function-analysis pass whose result is CFG-incremental, keyed
    /// by its type. Identical to [`Self::register_pass`] except the cached result
    /// carries the [`CfgIncremental`] repair hook, so the reshape `done()`-flush
    /// can offer it recorded edits instead of evicting it wholesale.
    pub fn register_cfg_pass<A>(&mut self, analysis: A)
    where
        A: FunctionAnalysis<'ctx, B>,
        A::Result: CfgIncremental<'ctx, B>,
    {
        let id = TypeId::of::<A>();
        let runner: FunctionRunner<'ctx, B> = Rc::new(move |function, am| {
            let result = analysis.run(function, am)?;
            Ok(CachedFunctionResult {
                result: Box::new(result),
                invalidate: invalidate_function_result::<B, A>,
                cfg_apply: Some(cfg_apply_result::<B, A::Result>),
            })
        });
        self.analyses.insert(id, runner);
    }

    /// Register `A` with its `Default` value unless an instance is already registered.
    ///
    /// The typed pipeline runner calls this from `FunctionAnalysisList::prefetch`
    /// so declared `Requires` entries never hit `IrError::AnalysisNotRegistered`.
    pub fn ensure_registered_default<A>(&mut self)
    where
        A: FunctionAnalysis<'ctx, B> + Default,
    {
        if !self.analyses.contains_key(&TypeId::of::<A>()) {
            self.register_pass(A::default());
        }
    }

    /// [`Self::ensure_registered_default`] for a CFG-incremental analysis: uses
    /// [`Self::register_cfg_pass`] so the cached result carries its
    /// [`CfgIncremental`] repair hook. A CFG analysis's
    /// [`PrefetchableAnalysis::ensure_registered`] calls this so that, once
    /// prefetched, it participates in framework-witnessed preservation.
    pub fn ensure_cfg_registered_default<A>(&mut self)
    where
        A: FunctionAnalysis<'ctx, B> + Default,
        A::Result: CfgIncremental<'ctx, B>,
    {
        if !self.analyses.contains_key(&TypeId::of::<A>()) {
            self.register_cfg_pass(A::default());
        }
    }

    /// Offer the recorded reshape `updates` to every cached CFG-incremental
    /// result for `function`, marking preserved in `pa` exactly those that
    /// repaired ([`RepairOutcome::Repaired`]) — the *witnessed* preservation
    /// step. A result that declines ([`RepairOutcome::PreferRecompute`], or has
    /// no hook) is left for `pa`'s floor to evict. Only the driver calls this,
    /// after a reshape pass and before [`Self::invalidate`].
    pub(crate) fn flush_cfg_updates(
        &mut self,
        function: FunctionView<'ctx, B>,
        updates: &[CfgUpdate],
        pa: &mut PreservedAnalyses,
    ) {
        let handle = function.as_function();
        let module_id = handle.module().id();
        let function_id = handle.id();
        for (key, cached) in &mut self.results {
            if key.0 != module_id || key.2 != function_id {
                continue;
            }
            let Some(apply) = cached.cfg_apply else {
                continue;
            };
            if apply(&mut *cached.result, updates, function) == RepairOutcome::Repaired {
                pa.preserve_type_id(key.1);
            }
        }
    }

    /// Fetch `function`'s result for analysis `A`, running the pass and caching
    /// the result on the first request. Errors with
    /// [`IrError::AnalysisNotRegistered`] if `A` was never registered.
    pub fn get_result<A, F>(&mut self, function: F) -> IrResult<&A::Result>
    where
        A: FunctionAnalysis<'ctx, B>,
        F: Into<FunctionView<'ctx, B>>,
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
        self.get_cached_result::<A, _>(function)
            .ok_or(IrError::AnalysisNotCached {
                name: type_name::<A>(),
            })
    }

    /// Return `function`'s already-cached result for `A`, or `None` if it has
    /// not been computed. Never runs the pass.
    pub fn get_cached_result<A, F>(&self, function: F) -> Option<&A::Result>
    where
        A: FunctionAnalysis<'ctx, B>,
        F: Into<FunctionView<'ctx, B>>,
    {
        let function = function.into();
        self.results
            .get(&function_key::<A, B>(function))?
            .result
            .downcast_ref::<A::Result>()
    }

    pub(crate) fn get_cached_result_by_type<A, R, F>(&self, function: F) -> Option<&R>
    where
        A: 'static,
        R: 'static,
        F: Into<FunctionView<'ctx, B>>,
    {
        let function = function.into();
        self.results
            .get(&function_key::<A, B>(function))?
            .result
            .downcast_ref::<R>()
    }

    /// Drop every cached result for `function` that `pa` does not preserve,
    /// consulting each result's own `invalidate` hook.
    pub fn invalidate<F>(&mut self, function: F, pa: &PreservedAnalyses) -> IrResult<()>
    where
        F: Into<FunctionView<'ctx, B>>,
    {
        let function = function.into();
        let function_handle = function.as_function();
        let module_id = function_handle.module().id();
        let function_id = function_handle.id();
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

    /// Propagate a module pass's preserved set `pa` down to the function
    /// analyses: clears every cached result when the module→function proxy is
    /// not preserved, otherwise invalidates each function's non-preserved
    /// results (a no-op when the whole `AllAnalysesOnFunction` set survives).
    pub fn invalidate_module(
        &mut self,
        module: ModuleView<'ctx, B>,
        pa: &PreservedAnalyses,
    ) -> IrResult<()> {
        if pa.are_all_preserved() {
            return Ok(());
        }

        let proxy = pa.checker::<FunctionAnalysisManagerModuleProxy>();
        if !(proxy.preserved() || proxy.preserved_set::<AllAnalysesOnModule>()) {
            self.clear();
            return Ok(());
        }

        if pa.all_analyses_in_set_preserved::<AllAnalysesOnFunction>() {
            return Ok(());
        }

        for function in module.iter_functions() {
            self.invalidate(function, pa)?;
        }
        Ok(())
    }

    /// Drop every cached function-analysis result.
    pub fn clear(&mut self) {
        self.results.clear();
    }

    /// Drop the cached result of analysis `A` for `function`, if present.
    pub fn clear_analysis<A, F>(&mut self, function: F)
    where
        A: FunctionAnalysis<'ctx, B>,
        F: Into<FunctionView<'ctx, B>>,
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
    /// Create an empty manager: no analyses registered and no cached results.
    pub fn new() -> Self {
        Self {
            analyses: HashMap::new(),
            results: HashMap::new(),
            instrumentation: None,
            _brand: PhantomData,
        }
    }

    /// Attach the instrumentation callbacks fired before and after each
    /// analysis run.
    pub fn set_instrumentation(&mut self, callbacks: PassInstrumentationCallbacks) {
        self.instrumentation = Some(callbacks);
    }

    /// Register a module-analysis pass instance, keyed by its type, so its
    /// result can be computed on demand by [`Self::get_result`].
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

    /// Register `A` with its `Default` value unless an instance is already registered.
    ///
    /// The typed pipeline runner calls this from `ModuleAnalysisList::prefetch`
    /// so declared `Requires` entries never hit `IrError::AnalysisNotRegistered`.
    pub fn ensure_registered_default<A>(&mut self)
    where
        A: ModuleAnalysis<'ctx, B> + Default,
    {
        if !self.analyses.contains_key(&TypeId::of::<A>()) {
            self.register_pass(A::default());
        }
    }

    /// Fetch the module's result for analysis `A`, running the pass and caching
    /// the result on the first request. Takes a verified module; errors with
    /// [`IrError::AnalysisNotRegistered`] if `A` was never registered.
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
        self.get_cached_result::<A, _>(module_view)
            .ok_or(IrError::AnalysisNotCached {
                name: type_name::<A>(),
            })
    }

    /// [`Self::get_result`] variant for callers that already hold a [`ModuleView`]
    /// rather than a `&Module<Verified>` (the typed pipeline runner keys its
    /// [`ModuleRunner`] by `ModuleView` already). Not part of the public API:
    /// [`ModuleAnalysisList::prefetch`] is the only caller.
    pub(crate) fn get_result_view<A>(&mut self, module: ModuleView<'ctx, B>) -> IrResult<&A::Result>
    where
        A: ModuleAnalysis<'ctx, B>,
    {
        let key = module_key::<A, B>(module);
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
        self.get_cached_result::<A, _>(module)
            .ok_or(IrError::AnalysisNotCached {
                name: type_name::<A>(),
            })
    }

    /// Return the module's already-cached result for `A`, or `None` if it has
    /// not been computed. Never runs the pass.
    pub fn get_cached_result<A, M>(&self, module: M) -> Option<&A::Result>
    where
        A: ModuleAnalysis<'ctx, B>,
        M: Into<ModuleView<'ctx, B>>,
    {
        let module = module.into();
        self.results
            .get(&module_key::<A, B>(module))?
            .result
            .downcast_ref::<A::Result>()
    }

    /// Drop every cached result for `module` that `pa` does not preserve,
    /// consulting each result's own `invalidate` hook.
    pub fn invalidate<M>(&mut self, module: M, pa: &PreservedAnalyses) -> IrResult<()>
    where
        M: Into<ModuleView<'ctx, B>>,
    {
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

    /// Drop every cached module-analysis result.
    pub fn clear(&mut self) {
        self.results.clear();
    }

    /// Drop the cached result of analysis `A` for `module`, if present.
    pub fn clear_analysis<A, M>(&mut self, module: M)
    where
        A: ModuleAnalysis<'ctx, B>,
        M: Into<ModuleView<'ctx, B>>,
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

/// One handle bundling the module + function analysis managers a pass driver
/// needs. Replaces threading `(&mut ModuleAnalysisManager, &mut FunctionAnalysisManager)`
/// by hand through every `run`.
pub struct Analyses<'ctx, B: ModuleBrand = Brand<'ctx>> {
    module: ModuleAnalysisManager<'ctx, B>,
    function: FunctionAnalysisManager<'ctx, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> Analyses<'ctx, B> {
    /// Create a bundle wrapping fresh, empty module and function analysis
    /// managers.
    pub fn new() -> Self {
        Self {
            module: ModuleAnalysisManager::new(),
            function: FunctionAnalysisManager::new(),
        }
    }

    /// Register a function analysis (delegates to the inner FAM's `register_pass`).
    pub fn register_function_analysis<A: FunctionAnalysis<'ctx, B>>(&mut self, analysis: A) {
        self.function.register_pass(analysis);
    }

    /// Register a module analysis.
    pub fn register_module_analysis<A: ModuleAnalysis<'ctx, B>>(&mut self, analysis: A) {
        self.module.register_pass(analysis);
    }

    /// Escape hatches for advanced callers who need a manager directly.
    pub fn function_manager(&self) -> &FunctionAnalysisManager<'ctx, B> {
        &self.function
    }

    /// Mutable access to the inner function analysis manager.
    pub fn function_manager_mut(&mut self) -> &mut FunctionAnalysisManager<'ctx, B> {
        &mut self.function
    }

    /// Shared access to the inner module analysis manager.
    pub fn module_manager(&self) -> &ModuleAnalysisManager<'ctx, B> {
        &self.module
    }

    /// Mutable access to the inner module analysis manager.
    pub fn module_manager_mut(&mut self) -> &mut ModuleAnalysisManager<'ctx, B> {
        &mut self.module
    }

    /// KEY split-borrow the module driver needs: both managers mutably at once.
    /// A single method returning both is how Rust lets you borrow two distinct
    /// fields mutably together (you cannot call two separate `&mut` methods).
    pub(crate) fn managers_mut(
        &mut self,
    ) -> (
        &mut ModuleAnalysisManager<'ctx, B>,
        &mut FunctionAnalysisManager<'ctx, B>,
    ) {
        (&mut self.module, &mut self.function)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> Default for Analyses<'ctx, B> {
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
    (function.module().id(), TypeId::of::<A>(), function.id())
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

/// Type-erased trampoline stored as a cached result's [`CfgApplyFn`]: downcast to
/// the concrete CFG-incremental result and offer it the recorded edits. Monotone
/// per analysis result type `R`; a downcast miss (never expected — the hook is
/// keyed to `R`) degrades safely to [`RepairOutcome::PreferRecompute`].
fn cfg_apply_result<'ctx, B, R>(
    result: &mut dyn Any,
    updates: &[CfgUpdate],
    function: FunctionView<'ctx, B>,
) -> RepairOutcome
where
    B: ModuleBrand + 'ctx,
    R: CfgIncremental<'ctx, B> + 'static,
{
    match result.downcast_mut::<R>() {
        Some(r) => r.apply_updates(updates, function),
        None => RepairOutcome::PreferRecompute,
    }
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

impl<'ctx, B: ModuleBrand + 'ctx> FunctionAnalysis<'ctx, B> for DominatorTreeAnalysis {
    type Result = DominatorTree;

    fn run(
        &self,
        function: FunctionView<'ctx, B>,
        _am: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Self::Result> {
        Ok(DominatorTree::new(function.as_function()))
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> PrefetchableAnalysis<'ctx, B> for DominatorTreeAnalysis {
    #[inline]
    fn ensure_registered(fam: &mut FunctionAnalysisManager<'ctx, B>) {
        // CFG-incremental: register WITH the repair hook so a prefetched dom
        // tree can be witnessed-preserved across a reshape instead of evicted.
        fam.ensure_cfg_registered_default::<Self>();
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionAnalysisResult<'ctx, B> for DominatorTree {
    fn invalidate(
        &mut self,
        _function: FunctionView<'ctx, B>,
        pa: &PreservedAnalyses,
        _inv: &mut FunctionAnalysisInvalidator<'_, 'ctx, B>,
    ) -> IrResult<bool> {
        let checker = pa.checker::<DominatorTreeAnalysis>();
        Ok(!(checker.preserved()
            || checker.preserved_set::<AllAnalysesOnFunction>()
            || checker.preserved_set::<CFGAnalyses>()))
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> CfgIncremental<'ctx, B> for DominatorTree {
    /// Repair the dominator tree after a batch of reshape edits. The repair is
    /// **correct-by-recompute**: it rebuilds the tree from the current (edited)
    /// CFG, which is trivially consistent with it, so the framework may keep the
    /// result — [`RepairOutcome::Repaired`]. This is what makes a reshape pass's
    /// dominator tree framework-*preserved* instead of evicted.
    ///
    /// The recorded `updates` are not yet used to do sub-linear work: a genuine
    /// incremental dominator update (LLVM SemiNCA-style, driven by the edge
    /// insert/delete list) is the documented perf follow-up in `future-work.md`.
    /// When it lands, a `debug_assert` comparing the incrementally-repaired tree
    /// to a from-scratch recompute (the property `repaired ≡ recomputed`) guards
    /// every flush; today the two are identical by construction.
    #[inline]
    fn apply_updates(
        &mut self,
        _updates: &[CfgUpdate],
        function: FunctionView<'ctx, B>,
    ) -> RepairOutcome {
        *self = DominatorTree::new(function.as_function());
        RepairOutcome::Repaired
    }

    #[inline]
    fn recompute(function: FunctionView<'ctx, B>) -> Self {
        DominatorTree::new(function.as_function())
    }
}

mod analysis_list_sealed {
    pub trait Sealed {}
}

/// Lifetime-free tuple schema of function-analysis markers, used as a pass's
/// `Requires` list. Mirrors the `FunctionParamList` tuple-schema shape
/// (`function_signature.rs`); no upstream analog — upstream requires runtime
/// registration (`AnalysisManager::registerPass`, `IR/PassManager.h`).
///
/// A `Requires` tuple that lists the same analysis type twice makes
/// [`AnalysisSelector::select`] ambiguous at the call site (two candidate
/// `I` index markers satisfy the same `A`), which surfaces as a trait-resolution
/// compile error rather than a runtime bug. That is acceptable: duplicate members
/// are a pathological spelling with no useful meaning.
pub trait FunctionAnalysisList<'ctx, B: ModuleBrand + 'ctx>: analysis_list_sealed::Sealed {
    /// Number of required analyses.
    const LEN: usize;

    /// Tuple of shared references to the members' cached results.
    type ResultRefs<'r>: Copy
    where
        'ctx: 'r;

    /// Register (if needed) and compute every member for `function`.
    fn prefetch(
        fam: &mut FunctionAnalysisManager<'ctx, B>,
        function: FunctionView<'ctx, B>,
    ) -> IrResult<()>;

    /// Collect cached references after [`Self::prefetch`]. The cache-miss branch
    /// is unreachable after a successful prefetch but reports
    /// [`IrError::AnalysisNotCached`] instead of panicking.
    fn collect<'r>(
        fam: &'r FunctionAnalysisManager<'ctx, B>,
        function: FunctionView<'ctx, B>,
    ) -> IrResult<Self::ResultRefs<'r>>;
}

impl analysis_list_sealed::Sealed for () {}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionAnalysisList<'ctx, B> for () {
    const LEN: usize = 0;
    type ResultRefs<'r>
        = ()
    where
        'ctx: 'r;

    fn prefetch(
        _fam: &mut FunctionAnalysisManager<'ctx, B>,
        _function: FunctionView<'ctx, B>,
    ) -> IrResult<()> {
        Ok(())
    }

    fn collect<'r>(
        _fam: &'r FunctionAnalysisManager<'ctx, B>,
        _function: FunctionView<'ctx, B>,
    ) -> IrResult<Self::ResultRefs<'r>> {
        Ok(())
    }
}

/// Positional index markers for [`AnalysisSelector`] / [`ModuleAnalysisSelector`].
/// Call sites never name them — the position is inferred from the analysis type.
#[derive(Debug, Clone, Copy)]
pub struct Idx0(());
/// Index marker for the analysis at `Requires` position 1 (see [`Idx0`]).
#[derive(Debug, Clone, Copy)]
pub struct Idx1(());
/// Index marker for the analysis at `Requires` position 2 (see [`Idx0`]).
#[derive(Debug, Clone, Copy)]
pub struct Idx2(());
/// Index marker for the analysis at `Requires` position 3 (see [`Idx0`]).
#[derive(Debug, Clone, Copy)]
pub struct Idx3(());
/// Index marker for the analysis at `Requires` position 4 (see [`Idx0`]).
#[derive(Debug, Clone, Copy)]
pub struct Idx4(());
/// Index marker for the analysis at `Requires` position 5 (see [`Idx0`]).
#[derive(Debug, Clone, Copy)]
pub struct Idx5(());
/// Index marker for the analysis at `Requires` position 6 (see [`Idx0`]).
#[derive(Debug, Clone, Copy)]
pub struct Idx6(());
/// Index marker for the analysis at `Requires` position 7 (see [`Idx0`]).
#[derive(Debug, Clone, Copy)]
pub struct Idx7(());

/// Compile-time membership proof: analysis `A` appears in this `Requires` list
/// at position `I` (inferred). The absent-impl case is the type error that
/// makes undeclared-analysis access unspellable in typed pass contexts.
#[diagnostic::on_unimplemented(
    message = "analysis `{A}` is not in this pass's `Requires` list `{Self}`",
    note = "add the analysis marker to `type Requires` on the pass, or use the erased pass path for ad-hoc queries"
)]
pub trait AnalysisSelector<'ctx, B: ModuleBrand + 'ctx, A: FunctionAnalysis<'ctx, B>, I>:
    FunctionAnalysisList<'ctx, B>
{
    /// Copy the selected member's reference out of the collected tuple.
    fn select<'r>(refs: &Self::ResultRefs<'r>) -> &'r A::Result
    where
        'ctx: 'r;
}

// The per-index `AnalysisSelector` impls need both the full member tuple
// (`$($all),+`, fixed across every impl) and one singled-out `$member`/`$idx`/`$slot`
// per impl. `macro_rules!` forbids re-expanding a repetition (`$($all),+`) from
// inside a different repetition group's iteration (`$($member: $idx . $slot),+`)
// even when the two groups share a length, so the selector impls are peeled off
// one at a time by recursion instead of a single `$(...)+ ` over all of them.
macro_rules! impl_function_analysis_list {
    ($len:literal; $($member:ident: $idx:ident . $slot:tt),+) => {
        impl<$($member),+> analysis_list_sealed::Sealed for ($($member,)+) {}

        impl<'ctx, B, $($member),+> FunctionAnalysisList<'ctx, B> for ($($member,)+)
        where
            B: ModuleBrand + 'ctx,
            $($member: PrefetchableAnalysis<'ctx, B>,)+
        {
            const LEN: usize = $len;
            type ResultRefs<'r>
                = ($(&'r $member::Result,)+)
            where
                'ctx: 'r;

            fn prefetch(
                fam: &mut FunctionAnalysisManager<'ctx, B>,
                function: FunctionView<'ctx, B>,
            ) -> IrResult<()> {
                $(
                    <$member as PrefetchableAnalysis<'ctx, B>>::ensure_registered(fam);
                    fam.get_result::<$member, _>(function)?;
                )+
                Ok(())
            }

            fn collect<'r>(
                fam: &'r FunctionAnalysisManager<'ctx, B>,
                function: FunctionView<'ctx, B>,
            ) -> IrResult<Self::ResultRefs<'r>> {
                Ok(($(
                    fam.get_cached_result::<$member, _>(function)
                        .ok_or(IrError::AnalysisNotCached {
                            name: type_name::<$member>(),
                        })?,
                )+))
            }
        }

        impl_function_analysis_selectors!([$($member),+]; $($member: $idx . $slot),+);
    };
}

macro_rules! impl_function_analysis_selectors {
    ([$($all:ident),+]; $head:ident: $hidx:ident . $hslot:tt $(, $member:ident: $idx:ident . $slot:tt)*) => {
        impl<'ctx, B, $($all),+> AnalysisSelector<'ctx, B, $head, $hidx>
            for ($($all,)+)
        where
            B: ModuleBrand + 'ctx,
            $($all: PrefetchableAnalysis<'ctx, B>,)+
        {
            fn select<'r>(refs: &Self::ResultRefs<'r>) -> &'r $head::Result
            where
                'ctx: 'r,
            {
                refs.$hslot
            }
        }

        impl_function_analysis_selectors!([$($all),+]; $($member: $idx . $slot),*);
    };
    ([$($all:ident),+]; ) => {};
}

impl_function_analysis_list!(1; A0: Idx0 . 0);
impl_function_analysis_list!(2; A0: Idx0 . 0, A1: Idx1 . 1);
impl_function_analysis_list!(3; A0: Idx0 . 0, A1: Idx1 . 1, A2: Idx2 . 2);
impl_function_analysis_list!(4; A0: Idx0 . 0, A1: Idx1 . 1, A2: Idx2 . 2, A3: Idx3 . 3);
impl_function_analysis_list!(5; A0: Idx0 . 0, A1: Idx1 . 1, A2: Idx2 . 2, A3: Idx3 . 3, A4: Idx4 . 4);
impl_function_analysis_list!(6; A0: Idx0 . 0, A1: Idx1 . 1, A2: Idx2 . 2, A3: Idx3 . 3, A4: Idx4 . 4, A5: Idx5 . 5);
impl_function_analysis_list!(7; A0: Idx0 . 0, A1: Idx1 . 1, A2: Idx2 . 2, A3: Idx3 . 3, A4: Idx4 . 4, A5: Idx5 . 5, A6: Idx6 . 6);
impl_function_analysis_list!(8; A0: Idx0 . 0, A1: Idx1 . 1, A2: Idx2 . 2, A3: Idx3 . 3, A4: Idx4 . 4, A5: Idx5 . 5, A6: Idx6 . 6, A7: Idx7 . 7);

/// Module-level mirror of [`FunctionAnalysisList`] over [`ModuleAnalysis`] /
/// [`ModuleAnalysisManager`] / [`ModuleView`]. Same duplicate-member caveat as
/// [`FunctionAnalysisList`]: a `Requires` tuple naming the same analysis twice
/// makes [`ModuleAnalysisSelector::select`] ambiguous, which is a compile error.
///
/// `impl_module_analysis_list!` below does not emit its own tuple
/// `analysis_list_sealed::Sealed` impl -- it relies on the unconstrained tuple
/// blanket already emitted by `impl_function_analysis_list!`, which seals every
/// tuple arity regardless of member kind. If that function-list blanket is ever
/// narrowed (e.g. bounded on `FunctionAnalysis`) or its arity coverage reduced,
/// this trait silently loses sealing for the arities it depends on.
pub trait ModuleAnalysisList<'ctx, B: ModuleBrand + 'ctx>: analysis_list_sealed::Sealed {
    /// Number of required analyses.
    const LEN: usize;

    /// Tuple of shared references to the members' cached results.
    type ResultRefs<'r>: Copy
    where
        'ctx: 'r;

    /// Register (if needed) and compute every member for `module`.
    fn prefetch(
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        module: ModuleView<'ctx, B>,
    ) -> IrResult<()>;

    /// Collect cached references after [`Self::prefetch`]. The cache-miss branch
    /// is unreachable after a successful prefetch but reports
    /// [`IrError::AnalysisNotCached`] instead of panicking.
    fn collect<'r>(
        mam: &'r ModuleAnalysisManager<'ctx, B>,
        module: ModuleView<'ctx, B>,
    ) -> IrResult<Self::ResultRefs<'r>>;
}

impl<'ctx, B: ModuleBrand + 'ctx> ModuleAnalysisList<'ctx, B> for () {
    const LEN: usize = 0;
    type ResultRefs<'r>
        = ()
    where
        'ctx: 'r;

    fn prefetch(
        _mam: &mut ModuleAnalysisManager<'ctx, B>,
        _module: ModuleView<'ctx, B>,
    ) -> IrResult<()> {
        Ok(())
    }

    fn collect<'r>(
        _mam: &'r ModuleAnalysisManager<'ctx, B>,
        _module: ModuleView<'ctx, B>,
    ) -> IrResult<Self::ResultRefs<'r>> {
        Ok(())
    }
}

/// Compile-time membership proof for [`ModuleAnalysisList`]: analysis `A`
/// appears in this `Requires` list at position `I` (inferred).
#[diagnostic::on_unimplemented(
    message = "analysis `{A}` is not in this pass's `Requires` list `{Self}`",
    note = "add the analysis marker to `type Requires` on the pass, or use the erased pass path for ad-hoc queries"
)]
pub trait ModuleAnalysisSelector<'ctx, B: ModuleBrand + 'ctx, A: ModuleAnalysis<'ctx, B>, I>:
    ModuleAnalysisList<'ctx, B>
{
    /// Copy the selected member's reference out of the collected tuple.
    fn select<'r>(refs: &Self::ResultRefs<'r>) -> &'r A::Result
    where
        'ctx: 'r;
}

// See `impl_function_analysis_selectors` above for why the selector impls are
// peeled off one at a time by recursion instead of a single `$(...)+ ` over
// all of them.
// NB: module `Requires` members still bound `+ Default` (auto-registered). The
// function side dropped this via `PrefetchableAnalysis` because it has real
// non-`Default` analyses; there are no concrete module analyses yet, so a
// mirror `PrefetchableModuleAnalysis` would be untestable dead machinery.
// Introduce it (same shape) when the first non-`Default` module analysis lands.
macro_rules! impl_module_analysis_list {
    ($len:literal; $($member:ident: $idx:ident . $slot:tt),+) => {
        impl<'ctx, B, $($member),+> ModuleAnalysisList<'ctx, B> for ($($member,)+)
        where
            B: ModuleBrand + 'ctx,
            $($member: ModuleAnalysis<'ctx, B> + Default,)+
        {
            const LEN: usize = $len;
            type ResultRefs<'r>
                = ($(&'r $member::Result,)+)
            where
                'ctx: 'r;

            fn prefetch(
                mam: &mut ModuleAnalysisManager<'ctx, B>,
                module: ModuleView<'ctx, B>,
            ) -> IrResult<()> {
                $(
                    mam.ensure_registered_default::<$member>();
                    mam.get_result_view::<$member>(module)?;
                )+
                Ok(())
            }

            fn collect<'r>(
                mam: &'r ModuleAnalysisManager<'ctx, B>,
                module: ModuleView<'ctx, B>,
            ) -> IrResult<Self::ResultRefs<'r>> {
                Ok(($(
                    mam.get_cached_result::<$member, _>(module)
                        .ok_or(IrError::AnalysisNotCached {
                            name: type_name::<$member>(),
                        })?,
                )+))
            }
        }

        impl_module_analysis_selectors!([$($member),+]; $($member: $idx . $slot),+);
    };
}

macro_rules! impl_module_analysis_selectors {
    ([$($all:ident),+]; $head:ident: $hidx:ident . $hslot:tt $(, $member:ident: $idx:ident . $slot:tt)*) => {
        impl<'ctx, B, $($all),+> ModuleAnalysisSelector<'ctx, B, $head, $hidx>
            for ($($all,)+)
        where
            B: ModuleBrand + 'ctx,
            $($all: ModuleAnalysis<'ctx, B> + Default,)+
        {
            fn select<'r>(refs: &Self::ResultRefs<'r>) -> &'r $head::Result
            where
                'ctx: 'r,
            {
                refs.$hslot
            }
        }

        impl_module_analysis_selectors!([$($all),+]; $($member: $idx . $slot),*);
    };
    ([$($all:ident),+]; ) => {};
}

impl_module_analysis_list!(1; A0: Idx0 . 0);
impl_module_analysis_list!(2; A0: Idx0 . 0, A1: Idx1 . 1);
impl_module_analysis_list!(3; A0: Idx0 . 0, A1: Idx1 . 1, A2: Idx2 . 2);
impl_module_analysis_list!(4; A0: Idx0 . 0, A1: Idx1 . 1, A2: Idx2 . 2, A3: Idx3 . 3);
impl_module_analysis_list!(5; A0: Idx0 . 0, A1: Idx1 . 1, A2: Idx2 . 2, A3: Idx3 . 3, A4: Idx4 . 4);
impl_module_analysis_list!(6; A0: Idx0 . 0, A1: Idx1 . 1, A2: Idx2 . 2, A3: Idx3 . 3, A4: Idx4 . 4, A5: Idx5 . 5);
impl_module_analysis_list!(7; A0: Idx0 . 0, A1: Idx1 . 1, A2: Idx2 . 2, A3: Idx3 . 3, A4: Idx4 . 4, A5: Idx5 . 5, A6: Idx6 . 6);
impl_module_analysis_list!(8; A0: Idx0 . 0, A1: Idx1 . 1, A2: Idx2 . 2, A3: Idx3 . 3, A4: Idx4 . 4, A5: Idx5 . 5, A6: Idx6 . 6, A7: Idx7 . 7);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Dyn, IRBuilder, Linkage, Module};

    /// llvmkit-specific type-machinery lock (no upstream analog): the analysis-list
    /// tuple schema prefetches, collects, and selects by type. Runtime behavior it
    /// wraps (getResult caching) ports `unittests/IR/PassManagerTest.cpp`.
    #[test]
    fn analysis_list_prefetch_collect_select() -> IrResult<()> {
        Module::with_new("analysis-list", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type_no_params(i32_ty, false);
            let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");
            let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
            b.build_ret(i32_ty.const_int(0_u32))?;
            m.verify_borrowed()?;

            let function: FunctionView<'_> = f.into();
            let mut fam = FunctionAnalysisManager::new();
            type Reqs = (DominatorTreeAnalysis,);
            <Reqs as FunctionAnalysisList<'_, _>>::prefetch(&mut fam, function)?;
            let refs = <Reqs as FunctionAnalysisList<'_, _>>::collect(&fam, function)?;
            // `B` is pinned explicitly here: unlike `prefetch`/`collect`, `select`'s
            // only argument is `Self::ResultRefs<'r>`, whose concrete type
            // (`&DominatorTree`) doesn't mention `B`, so `_` has nothing to infer from.
            let dt: &DominatorTree =
                <Reqs as AnalysisSelector<'_, Brand<'_>, DominatorTreeAnalysis, Idx0>>::select(
                    &refs,
                );
            let entry_view = function
                .entry_block()
                .map(|bb| dt.is_reachable_from_entry(bb));
            assert_eq!(entry_view, Some(true));
            Ok(())
        })
    }

    /// The dominator tree's [`CfgIncremental`] hook repairs the tree after a
    /// reshape edit (correct-by-recompute) and returns [`RepairOutcome::Repaired`]
    /// so the framework keeps it. Property: a stale cached tree, offered the
    /// edits via `apply_updates`, answers reachability EXACTLY like a
    /// from-scratch recompute of the edited CFG. llvmkit-specific
    /// witnessed-preservation plumbing (no upstream analog: LLVM hand-feeds
    /// `DomTreeUpdater` and trusts author-supplied edits).
    #[test]
    fn dominator_tree_repairs_to_match_recompute() -> IrResult<()> {
        use crate::CfgUpdate;
        Module::with_new("domtree-repair", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type_no_params(i32_ty, false);
            let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");
            let next = f.append_basic_block(&m, "next");
            let entry_id = entry.as_value().id;
            let next_id = next.as_value().id;
            let next_label = next.label();

            // entry: br next    next: ret 0
            let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
            b.build_br(next.label())?;
            let b2 = IRBuilder::new_for::<Dyn>(&m).position_at_end(next);
            b2.build_ret(i32_ty.const_int(0_u32))?;

            let function: FunctionView<'_> = f.into();

            // Cache a dom tree while `next` is still reachable.
            let mut dt = DominatorTree::new(function.as_function());
            assert!(dt.is_reachable_from_entry(next_label));

            // Edit the CFG: split the entry before its terminator, moving the
            // `br next` (and the only edge into `next`) into a fresh block that
            // nothing reaches — so `next` is now unreachable.
            let entry_bb = function.entry_block().expect("definition").as_basic_block();
            let terminator = entry_bb.terminator().expect("terminated");
            let new_bb = entry_bb.split_at(&m, &terminator, "entry.split")?;
            let updates = [
                CfgUpdate::delete(entry_id, next_id),
                CfgUpdate::insert(new_bb.as_value().id, next_id),
            ];

            // Repairing the stale cached tree returns Repaired and yields the
            // same answer as a fresh recompute: `next` unreachable.
            assert_eq!(
                dt.apply_updates(&updates, function),
                RepairOutcome::Repaired
            );
            let fresh = DominatorTree::new(function.as_function());
            assert_eq!(
                dt.is_reachable_from_entry(next_label),
                fresh.is_reachable_from_entry(next_label)
            );
            assert!(!dt.is_reachable_from_entry(next_label));
            Ok(())
        })
    }

    /// A deliberately NON-`Default` function analysis: it carries configuration,
    /// so a result can only come from a pre-registered instance. Used to prove a
    /// `Requires` list no longer bounds its members `Default`.
    #[derive(Clone, Copy)]
    struct ThresholdAnalysis {
        threshold: u32,
    }
    struct ThresholdResult {
        threshold: u32,
    }
    impl<'ctx, B: ModuleBrand + 'ctx> FunctionAnalysisResult<'ctx, B> for ThresholdResult {}
    impl<'ctx, B: ModuleBrand + 'ctx> FunctionAnalysis<'ctx, B> for ThresholdAnalysis {
        type Result = ThresholdResult;
        fn run(
            &self,
            _function: FunctionView<'ctx, B>,
            _am: &mut FunctionAnalysisManager<'ctx, B>,
        ) -> IrResult<Self::Result> {
            Ok(ThresholdResult {
                threshold: self.threshold,
            })
        }
    }
    impl<'ctx, B: ModuleBrand + 'ctx> PrefetchableAnalysis<'ctx, B> for ThresholdAnalysis {
        fn ensure_registered(_fam: &mut FunctionAnalysisManager<'ctx, B>) {
            // No-op: a non-`Default` analysis must be pre-registered by the
            // caller (there is nothing to auto-construct).
        }
    }

    /// A `Requires` list member need not be `Default`: a parameterized analysis
    /// works as long as the caller pre-registered a configured instance, and the
    /// prefetched result reflects THAT instance's config. Without the
    /// pre-registration the prefetch reports `AnalysisNotRegistered` — proving
    /// the `PrefetchableAnalysis` no-op does not silently auto-construct.
    /// llvmkit-specific type-machinery lock (no upstream analog).
    #[test]
    fn requires_without_default_uses_registered_instance() -> IrResult<()> {
        Module::with_new("requires-no-default", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type_no_params(i32_ty, false);
            let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");
            let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
            b.build_ret(i32_ty.const_int(0_u32))?;

            let function: FunctionView<'_> = f.into();
            type Reqs = (ThresholdAnalysis,);

            // Without pre-registration, prefetch fails: the no-op
            // `ensure_registered` does not auto-construct a non-`Default` analysis.
            let mut empty = FunctionAnalysisManager::new();
            assert!(matches!(
                <Reqs as FunctionAnalysisList<'_, _>>::prefetch(&mut empty, function),
                Err(IrError::AnalysisNotRegistered { .. })
            ));

            // With a configured instance pre-registered, the Requires list
            // prefetches/collects/selects it and the result carries the config.
            let mut fam = FunctionAnalysisManager::new();
            fam.register_pass(ThresholdAnalysis { threshold: 42 });
            <Reqs as FunctionAnalysisList<'_, _>>::prefetch(&mut fam, function)?;
            let refs = <Reqs as FunctionAnalysisList<'_, _>>::collect(&fam, function)?;
            let result: &ThresholdResult =
                <Reqs as AnalysisSelector<'_, Brand<'_>, ThresholdAnalysis, Idx0>>::select(&refs);
            assert_eq!(result.threshold, 42);
            Ok(())
        })
    }
}
