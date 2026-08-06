//! Minimal LLVM-new-PM-style analysis substrate. Mirrors the
//! `Analysis.h` / `PassManager.h` pieces needed by llvmkit's first
//! function and module analyses.

#![deny(missing_docs)]

use std::any::{Any, TypeId, type_name};
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::rc::Rc;

use super::module::{Module, Verified};
use crate::cfg_update::CfgUpdate;
use crate::dominator_tree::{DominatorTree, DominatorTreeAnalysis};
use crate::module::{ModuleBrand, ModuleId, ModuleView};
use crate::pass_context::FunctionView;
use crate::pass_instrumentation::PassInstrumentationCallbacks;
use crate::value::{IsValue, ValueSlot};
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
pub struct CfgAnalyses;

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
pub trait ModuleAnalysis<'ctx, B: ModuleBrand>: 'static {
    /// The cached result value this analysis produces.
    type Result: ModuleAnalysisResult<'ctx, B> + 'static;

    /// Compute the analysis over `module`, using `am` to fetch any analyses it
    /// depends on.
    ///
    /// The view region `'v` is the *caller's*, chosen per call and only required
    /// to be outlived by the manager's `'ctx`. A driver that owns its module can
    /// therefore mint the view at its own borrow; `Self::Result: 'static`, so
    /// nothing borrowed at `'v` can escape into the cache.
    fn run<'v>(
        &self,
        module: ModuleView<'v, B>,
        am: &mut ModuleAnalysisManager<'ctx, B>,
    ) -> IrResult<Self::Result>
    where
        'ctx: 'v;
}

/// Cached module-analysis result.
pub trait ModuleAnalysisResult<'ctx, B: ModuleBrand>: 'static {
    /// Return `true` when this result should be invalidated.
    fn invalidate<'v>(
        &mut self,
        _module: ModuleView<'v, B>,
        _pa: &PreservedAnalyses,
        _inv: &mut ModuleAnalysisInvalidator<'_, 'ctx, B>,
    ) -> IrResult<bool>
    where
        'ctx: 'v,
    {
        Ok(true)
    }
}

/// A function analysis pass.
pub trait FunctionAnalysis<'ctx, B: ModuleBrand>: 'static {
    /// The cached result value this analysis produces.
    type Result: FunctionAnalysisResult<'ctx, B> + 'static;

    /// Compute the analysis over `function`, using `am` to fetch any analyses
    /// it depends on.
    ///
    /// The view region `'v` is the *caller's*, chosen per call and only required
    /// to be outlived by the manager's `'ctx` — see [`ModuleAnalysis::run`].
    fn run<'v>(
        &self,
        function: FunctionView<'v, B>,
        am: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Self::Result>
    where
        'ctx: 'v;
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
pub trait PrefetchableAnalysis<'ctx, B: ModuleBrand>: FunctionAnalysis<'ctx, B> {
    /// Ensure this analysis is registered in `fam`, so a following `result`
    /// cannot fail with [`IrError::AnalysisNotRegistered`].
    fn ensure_registered(fam: &mut FunctionAnalysisManager<'ctx, B>);
}

/// Cached function-analysis result.
pub trait FunctionAnalysisResult<'ctx, B: ModuleBrand>: 'static {
    /// Return `true` when this result should be invalidated.
    fn invalidate<'v>(
        &mut self,
        _function: FunctionView<'v, B>,
        _pa: &PreservedAnalyses,
        _inv: &mut FunctionAnalysisInvalidator<'_, 'ctx, B>,
    ) -> IrResult<bool>
    where
        'ctx: 'v,
    {
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
pub trait CfgIncremental<'ctx, B: ModuleBrand>: Sized {
    /// Fold the recorded `updates` (in the order they were performed over
    /// `function`) into this cached result. Return [`RepairOutcome::Repaired`]
    /// only if the result is now fully consistent with the edited CFG;
    /// otherwise return [`RepairOutcome::PreferRecompute`] and the framework
    /// recomputes (via [`Self::recompute`]) or evicts.
    fn apply_updates<'v>(
        &mut self,
        updates: &[CfgUpdate],
        function: FunctionView<'v, B>,
    ) -> RepairOutcome
    where
        'ctx: 'v;

    /// Recompute this analysis from scratch over `function`'s current CFG. The
    /// framework calls this whenever [`Self::apply_updates`] returns
    /// [`RepairOutcome::PreferRecompute`], so a mid-pass read of a CFG analysis
    /// after a reshape edit still yields a *correct* result rather than a stale
    /// cached one. Must equal a fresh construction of the analysis.
    fn recompute<'v>(function: FunctionView<'v, B>) -> Self
    where
        'ctx: 'v;
}

/// Type-erased per-analysis operations, stored once per registered function
/// analysis and cloned onto each cached result.
///
/// This is a trait object rather than the `Rc<dyn Fn(..)>` / `fn(..)` pointers
/// it replaces because those pin the *view* region to the manager's `'ctx`: a
/// closure's argument types are fixed, and an `fn` pointer's argument lifetime
/// is contravariant (a `fn(FunctionView<'ctx, _>)` cannot be called with a
/// shorter view). A trait whose methods are generic over `'v` *with `'ctx: 'v`
/// in scope* is object-safe (lifetime generics are allowed on `dyn` methods) and
/// keeps `B: 'v` provable from `B: 'ctx`. That is what lets a driver holding an
/// owned module feed the manager a view minted at its own borrow.
trait FunctionAnalysisOps<'ctx, B: ModuleBrand + 'ctx> {
    /// Run the analysis and box its result together with a clone of these ops.
    fn run_erased<'v>(
        &self,
        function: FunctionView<'v, B>,
        am: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Box<dyn Any>>
    where
        'ctx: 'v;

    /// Consult the cached result's own invalidation hook.
    fn invalidate_erased<'v>(
        &self,
        result: &mut dyn Any,
        function: FunctionView<'v, B>,
        pa: &PreservedAnalyses,
        snapshot: &FunctionAnalysisSnapshot,
    ) -> IrResult<bool>
    where
        'ctx: 'v;

    /// Offer recorded CFG edits to a [`CfgIncremental`] result. `None` when this
    /// analysis's result does not implement the hook (a value analysis).
    fn cfg_apply_erased<'v>(
        &self,
        _result: &mut dyn Any,
        _updates: &[CfgUpdate],
        _function: FunctionView<'v, B>,
    ) -> Option<RepairOutcome>
    where
        'ctx: 'v,
    {
        None
    }
}

/// Module-level mirror of [`FunctionAnalysisOps`].
trait ModuleAnalysisOps<'ctx, B: ModuleBrand + 'ctx> {
    fn run_erased<'v>(
        &self,
        module: ModuleView<'v, B>,
        am: &mut ModuleAnalysisManager<'ctx, B>,
    ) -> IrResult<Box<dyn Any>>
    where
        'ctx: 'v;

    fn invalidate_erased<'v>(
        &self,
        result: &mut dyn Any,
        module: ModuleView<'v, B>,
        pa: &PreservedAnalyses,
        snapshot: &ModuleAnalysisSnapshot,
    ) -> IrResult<bool>
    where
        'ctx: 'v;
}

/// [`FunctionAnalysisOps`] carrier for a plain (non-CFG-incremental) analysis.
struct FunctionOpsOf<A>(A);

impl<'ctx, B, A> FunctionAnalysisOps<'ctx, B> for FunctionOpsOf<A>
where
    B: ModuleBrand + 'ctx,
    A: FunctionAnalysis<'ctx, B>,
{
    fn run_erased<'v>(
        &self,
        function: FunctionView<'v, B>,
        am: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Box<dyn Any>>
    where
        'ctx: 'v,
    {
        Ok(Box::new(self.0.run(function, am)?))
    }

    fn invalidate_erased<'v>(
        &self,
        result: &mut dyn Any,
        function: FunctionView<'v, B>,
        pa: &PreservedAnalyses,
        snapshot: &FunctionAnalysisSnapshot,
    ) -> IrResult<bool>
    where
        'ctx: 'v,
    {
        invalidate_function_result::<B, A>(result, function, pa, snapshot)
    }
}

/// [`FunctionAnalysisOps`] carrier for an analysis whose result is
/// [`CfgIncremental`] — the only difference is that `cfg_apply_erased` is live.
struct CfgFunctionOpsOf<A>(A);

impl<'ctx, B, A> FunctionAnalysisOps<'ctx, B> for CfgFunctionOpsOf<A>
where
    B: ModuleBrand + 'ctx,
    A: FunctionAnalysis<'ctx, B>,
    A::Result: CfgIncremental<'ctx, B>,
{
    fn run_erased<'v>(
        &self,
        function: FunctionView<'v, B>,
        am: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Box<dyn Any>>
    where
        'ctx: 'v,
    {
        Ok(Box::new(self.0.run(function, am)?))
    }

    fn invalidate_erased<'v>(
        &self,
        result: &mut dyn Any,
        function: FunctionView<'v, B>,
        pa: &PreservedAnalyses,
        snapshot: &FunctionAnalysisSnapshot,
    ) -> IrResult<bool>
    where
        'ctx: 'v,
    {
        invalidate_function_result::<B, A>(result, function, pa, snapshot)
    }

    fn cfg_apply_erased<'v>(
        &self,
        result: &mut dyn Any,
        updates: &[CfgUpdate],
        function: FunctionView<'v, B>,
    ) -> Option<RepairOutcome>
    where
        'ctx: 'v,
    {
        Some(cfg_apply_result::<B, A::Result>(result, updates, function))
    }
}

/// [`ModuleAnalysisOps`] carrier.
struct ModuleOpsOf<A>(A);

impl<'ctx, B, A> ModuleAnalysisOps<'ctx, B> for ModuleOpsOf<A>
where
    B: ModuleBrand + 'ctx,
    A: ModuleAnalysis<'ctx, B>,
{
    fn run_erased<'v>(
        &self,
        module: ModuleView<'v, B>,
        am: &mut ModuleAnalysisManager<'ctx, B>,
    ) -> IrResult<Box<dyn Any>>
    where
        'ctx: 'v,
    {
        Ok(Box::new(self.0.run(module, am)?))
    }

    fn invalidate_erased<'v>(
        &self,
        result: &mut dyn Any,
        module: ModuleView<'v, B>,
        pa: &PreservedAnalyses,
        snapshot: &ModuleAnalysisSnapshot,
    ) -> IrResult<bool>
    where
        'ctx: 'v,
    {
        invalidate_module_result::<B, A>(result, module, pa, snapshot)
    }
}

type FunctionOps<'ctx, B> = Rc<dyn FunctionAnalysisOps<'ctx, B> + 'ctx>;
type ModuleOps<'ctx, B> = Rc<dyn ModuleAnalysisOps<'ctx, B> + 'ctx>;

struct CachedFunctionResult<'ctx, B: ModuleBrand + 'ctx> {
    result: Box<dyn Any>,
    ops: FunctionOps<'ctx, B>,
}

struct CachedModuleResult<'ctx, B: ModuleBrand + 'ctx> {
    result: Box<dyn Any>,
    ops: ModuleOps<'ctx, B>,
}

#[derive(Clone)]
struct FunctionAnalysisSnapshot {
    cached: HashSet<(ModuleId, TypeId, ValueSlot)>,
}

#[derive(Clone)]
struct ModuleAnalysisSnapshot {
    cached: HashSet<(TypeId, ModuleId)>,
}

/// Invalidator passed to function-analysis results.
///
/// Holds the *key* of the function being invalidated rather than a
/// [`FunctionView`], so the struct carries no view region: the view handed to
/// [`FunctionAnalysisResult::invalidate`] lives at the caller-chosen `'v` (which
/// an owned module mints at its borrow), while this invalidator stays at the
/// manager's `'ctx`.
pub struct FunctionAnalysisInvalidator<'a, 'ctx, B: ModuleBrand> {
    module_id: ModuleId,
    function_slot: ValueSlot,
    pa: &'a PreservedAnalyses,
    snapshot: &'a FunctionAnalysisSnapshot,
    _brand: PhantomData<fn(B) -> B>,
    _ctx: PhantomData<&'ctx ()>,
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
        let key = (self.module_id, TypeId::of::<A>(), self.function_slot);
        if !self.snapshot.cached.contains(&key) {
            return Err(IrError::AnalysisNotCached {
                name: type_name::<A>(),
            });
        }
        let checker = self.pa.checker::<A>();
        Ok(!(checker.preserved() || checker.preserved_set::<AllAnalysesOnFunction>()))
    }
}

/// Invalidator passed to module-analysis results. Holds the module *key* rather
/// than a [`ModuleView`], for the same reason as
/// [`FunctionAnalysisInvalidator`].
pub struct ModuleAnalysisInvalidator<'a, 'ctx, B: ModuleBrand> {
    module_id: ModuleId,
    pa: &'a PreservedAnalyses,
    snapshot: &'a ModuleAnalysisSnapshot,
    _brand: PhantomData<fn(B) -> B>,
    _ctx: PhantomData<&'ctx ()>,
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
        let key = (TypeId::of::<A>(), self.module_id);
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
pub struct FunctionAnalysisManager<'ctx, B: ModuleBrand> {
    analyses: HashMap<TypeId, FunctionOps<'ctx, B>>,
    results: HashMap<(ModuleId, TypeId, ValueSlot), CachedFunctionResult<'ctx, B>>,
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
    /// result can be computed on demand by [`Self::result`].
    pub fn register_pass<A>(&mut self, analysis: A)
    where
        A: FunctionAnalysis<'ctx, B>,
    {
        let id = TypeId::of::<A>();
        let ops: FunctionOps<'ctx, B> = Rc::new(FunctionOpsOf(analysis));
        self.analyses.insert(id, ops);
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
        let ops: FunctionOps<'ctx, B> = Rc::new(CfgFunctionOpsOf(analysis));
        self.analyses.insert(id, ops);
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
    pub(crate) fn flush_cfg_updates<'v>(
        &mut self,
        function: FunctionView<'v, B>,
        updates: &[CfgUpdate],
        pa: &mut PreservedAnalyses,
    ) where
        'ctx: 'v,
    {
        let handle = function.as_function();
        let module_id = handle.module().id();
        let function_id = handle.slot();
        for (key, cached) in &mut self.results {
            if key.0 != module_id || key.2 != function_id {
                continue;
            }
            let ops = Rc::clone(&cached.ops);
            if ops.cfg_apply_erased(&mut *cached.result, updates, function)
                == Some(RepairOutcome::Repaired)
            {
                pa.preserve_type_id(key.1);
            }
        }
    }

    /// Fetch `function`'s result for analysis `A`, running the pass and caching
    /// the result on the first request. Errors with
    /// [`IrError::AnalysisNotRegistered`] if `A` was never registered.
    pub fn result<'v, A, F>(&mut self, function: F) -> IrResult<&A::Result>
    where
        A: FunctionAnalysis<'ctx, B>,
        F: Into<FunctionView<'v, B>>,
        'ctx: 'v,
    {
        let function = function.into();
        let key = function_key::<A, B>(function);
        if !self.results.contains_key(&key) {
            let Some(ops) = self.analyses.get(&key.1).cloned() else {
                return Err(IrError::AnalysisNotRegistered {
                    name: type_name::<A>(),
                });
            };
            if let Some(callbacks) = &self.instrumentation {
                callbacks.run_before_analysis(type_name::<A>());
            }
            let result = ops.run_erased(function, self)?;
            self.results
                .insert(key, CachedFunctionResult { result, ops });
            if let Some(callbacks) = &self.instrumentation {
                callbacks.run_after_analysis(type_name::<A>());
            }
        }
        self.cached_result::<A, _>(function)
            .ok_or(IrError::AnalysisNotCached {
                name: type_name::<A>(),
            })
    }

    /// Return `function`'s already-cached result for `A`, or `None` if it has
    /// not been computed. Never runs the pass.
    pub fn cached_result<'v, A, F>(&self, function: F) -> Option<&A::Result>
    where
        A: FunctionAnalysis<'ctx, B>,
        F: Into<FunctionView<'v, B>>,
        'ctx: 'v,
    {
        let function = function.into();
        self.results
            .get(&function_key::<A, B>(function))?
            .result
            .downcast_ref::<A::Result>()
    }

    pub(crate) fn cached_result_by_type<'v, A, R, F>(&self, function: F) -> Option<&R>
    where
        A: 'static,
        R: 'static,
        F: Into<FunctionView<'v, B>>,
        'ctx: 'v,
    {
        let function = function.into();
        self.results
            .get(&function_key::<A, B>(function))?
            .result
            .downcast_ref::<R>()
    }

    /// Drop every cached result for `function` that `pa` does not preserve,
    /// consulting each result's own `invalidate` hook.
    pub fn invalidate<'v, F>(&mut self, function: F, pa: &PreservedAnalyses) -> IrResult<()>
    where
        F: Into<FunctionView<'v, B>>,
        'ctx: 'v,
    {
        let function = function.into();
        let function_handle = function.as_function();
        let module_id = function_handle.module().id();
        let function_id = function_handle.slot();
        let snapshot = FunctionAnalysisSnapshot {
            cached: self.results.keys().copied().collect(),
        };
        let mut dead = Vec::new();
        for (key, cached) in &mut self.results {
            if key.0 == module_id
                && key.2 == function_id
                && Rc::clone(&cached.ops).invalidate_erased(
                    &mut *cached.result,
                    function,
                    pa,
                    &snapshot,
                )?
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
    pub fn invalidate_module<'v>(
        &mut self,
        module: ModuleView<'v, B>,
        pa: &PreservedAnalyses,
    ) -> IrResult<()>
    where
        'ctx: 'v,
    {
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

        for function in module.functions() {
            self.invalidate(function, pa)?;
        }
        Ok(())
    }

    /// Drop every cached function-analysis result.
    pub fn clear(&mut self) {
        self.results.clear();
    }

    /// Drop the cached result of analysis `A` for `function`, if present.
    pub fn clear_analysis<'v, A, F>(&mut self, function: F)
    where
        A: FunctionAnalysis<'ctx, B>,
        F: Into<FunctionView<'v, B>>,
        'ctx: 'v,
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
pub struct ModuleAnalysisManager<'ctx, B: ModuleBrand> {
    analyses: HashMap<TypeId, ModuleOps<'ctx, B>>,
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
    /// result can be computed on demand by [`Self::result`].
    pub fn register_pass<A>(&mut self, analysis: A)
    where
        A: ModuleAnalysis<'ctx, B>,
    {
        let id = TypeId::of::<A>();
        let ops: ModuleOps<'ctx, B> = Rc::new(ModuleOpsOf(analysis));
        self.analyses.insert(id, ops);
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
    pub fn result<'v, A>(&mut self, module: &'v Module<B, Verified>) -> IrResult<&A::Result>
    where
        A: ModuleAnalysis<'ctx, B>,
        'ctx: 'v,
    {
        let module_view = module.as_view();
        self.result_view::<A>(module_view)
    }

    /// [`Self::result`] variant for callers that already hold a [`ModuleView`]
    /// rather than a `&Module<Verified>` (the typed pipeline runner keys its
    /// [`ModuleRunner`] by `ModuleView` already). Not part of the public API:
    /// [`ModuleAnalysisList::prefetch`] is the only caller.
    pub(crate) fn result_view<'v, A>(&mut self, module: ModuleView<'v, B>) -> IrResult<&A::Result>
    where
        A: ModuleAnalysis<'ctx, B>,
        'ctx: 'v,
    {
        let key = module_key::<A, B>(module);
        if !self.results.contains_key(&key) {
            let Some(ops) = self.analyses.get(&key.0).cloned() else {
                return Err(IrError::AnalysisNotRegistered {
                    name: type_name::<A>(),
                });
            };
            if let Some(callbacks) = &self.instrumentation {
                callbacks.run_before_analysis(type_name::<A>());
            }
            let result = ops.run_erased(module, self)?;
            self.results.insert(key, CachedModuleResult { result, ops });
            if let Some(callbacks) = &self.instrumentation {
                callbacks.run_after_analysis(type_name::<A>());
            }
        }
        self.cached_result::<A, _>(module)
            .ok_or(IrError::AnalysisNotCached {
                name: type_name::<A>(),
            })
    }

    /// Return the module's already-cached result for `A`, or `None` if it has
    /// not been computed. Never runs the pass.
    pub fn cached_result<'v, A, M>(&self, module: M) -> Option<&A::Result>
    where
        A: ModuleAnalysis<'ctx, B>,
        M: Into<ModuleView<'v, B>>,
        'ctx: 'v,
    {
        let module = module.into();
        self.results
            .get(&module_key::<A, B>(module))?
            .result
            .downcast_ref::<A::Result>()
    }

    /// Drop every cached result for `module` that `pa` does not preserve,
    /// consulting each result's own `invalidate` hook.
    pub fn invalidate<'v, M>(&mut self, module: M, pa: &PreservedAnalyses) -> IrResult<()>
    where
        M: Into<ModuleView<'v, B>>,
        'ctx: 'v,
    {
        let module = module.into();
        let module_id = module.id();
        let snapshot = ModuleAnalysisSnapshot {
            cached: self.results.keys().copied().collect(),
        };
        let mut dead = Vec::new();
        for (key, cached) in &mut self.results {
            if key.1 == module_id
                && Rc::clone(&cached.ops).invalidate_erased(
                    &mut *cached.result,
                    module,
                    pa,
                    &snapshot,
                )?
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
    pub fn clear_analysis<'v, A, M>(&mut self, module: M)
    where
        A: ModuleAnalysis<'ctx, B>,
        M: Into<ModuleView<'v, B>>,
        'ctx: 'v,
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
pub struct Analyses<'ctx, B: ModuleBrand> {
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

fn function_key<'ctx, A, B>(function: FunctionView<'ctx, B>) -> (ModuleId, TypeId, ValueSlot)
where
    A: 'static,
    B: ModuleBrand + 'ctx,
{
    let function = function.as_function();
    (function.module().id(), TypeId::of::<A>(), function.slot())
}

fn module_key<'ctx, A, B>(module: ModuleView<'ctx, B>) -> (TypeId, ModuleId)
where
    A: 'static,
    B: ModuleBrand + 'ctx,
{
    (TypeId::of::<A>(), module.id())
}

fn invalidate_function_result<'v, 'ctx, B, A>(
    result: &mut dyn Any,
    function: FunctionView<'v, B>,
    pa: &PreservedAnalyses,
    snapshot: &FunctionAnalysisSnapshot,
) -> IrResult<bool>
where
    B: ModuleBrand + 'ctx,
    A: FunctionAnalysis<'ctx, B>,
    'ctx: 'v,
{
    let Some(result) = result.downcast_mut::<A::Result>() else {
        return Ok(true);
    };
    let handle = function.as_function();
    let mut invalidator = FunctionAnalysisInvalidator::<'_, 'ctx, B> {
        module_id: handle.module().id(),
        function_slot: handle.slot(),
        pa,
        snapshot,
        _brand: PhantomData,
        _ctx: PhantomData,
    };
    result.invalidate(function, pa, &mut invalidator)
}

/// Type-erased trampoline behind [`FunctionAnalysisOps::cfg_apply_erased`]: downcast to
/// the concrete CFG-incremental result and offer it the recorded edits. Monotone
/// per analysis result type `R`; a downcast miss (never expected — the hook is
/// keyed to `R`) degrades safely to [`RepairOutcome::PreferRecompute`].
fn cfg_apply_result<'v, 'ctx, B, R>(
    result: &mut dyn Any,
    updates: &[CfgUpdate],
    function: FunctionView<'v, B>,
) -> RepairOutcome
where
    B: ModuleBrand + 'ctx,
    R: CfgIncremental<'ctx, B> + 'static,
    'ctx: 'v,
{
    match result.downcast_mut::<R>() {
        Some(r) => r.apply_updates(updates, function),
        None => RepairOutcome::PreferRecompute,
    }
}

fn invalidate_module_result<'v, 'ctx, B, A>(
    result: &mut dyn Any,
    module: ModuleView<'v, B>,
    pa: &PreservedAnalyses,
    snapshot: &ModuleAnalysisSnapshot,
) -> IrResult<bool>
where
    B: ModuleBrand + 'ctx,
    A: ModuleAnalysis<'ctx, B>,
    'ctx: 'v,
{
    let Some(result) = result.downcast_mut::<A::Result>() else {
        return Ok(true);
    };
    let mut invalidator = ModuleAnalysisInvalidator::<'_, 'ctx, B> {
        module_id: module.id(),
        pa,
        snapshot,
        _brand: PhantomData,
        _ctx: PhantomData,
    };
    result.invalidate(module, pa, &mut invalidator)
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionAnalysis<'ctx, B> for DominatorTreeAnalysis {
    type Result = DominatorTree;

    fn run<'v>(
        &self,
        function: FunctionView<'v, B>,
        _am: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Self::Result>
    where
        'ctx: 'v,
    {
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
    fn invalidate<'v>(
        &mut self,
        _function: FunctionView<'v, B>,
        pa: &PreservedAnalyses,
        _inv: &mut FunctionAnalysisInvalidator<'_, 'ctx, B>,
    ) -> IrResult<bool>
    where
        'ctx: 'v,
    {
        let checker = pa.checker::<DominatorTreeAnalysis>();
        Ok(!(checker.preserved()
            || checker.preserved_set::<AllAnalysesOnFunction>()
            || checker.preserved_set::<CfgAnalyses>()))
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
    fn apply_updates<'v>(
        &mut self,
        _updates: &[CfgUpdate],
        function: FunctionView<'v, B>,
    ) -> RepairOutcome
    where
        'ctx: 'v,
    {
        *self = DominatorTree::new(function.as_function());
        RepairOutcome::Repaired
    }

    #[inline]
    fn recompute<'v>(function: FunctionView<'v, B>) -> Self
    where
        'ctx: 'v,
    {
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
    ///
    /// The view region `'v` is the driver's, not the manager's: a driver that
    /// owns its module can only mint a view at its own borrow.
    fn prefetch<'v>(
        fam: &mut FunctionAnalysisManager<'ctx, B>,
        function: FunctionView<'v, B>,
    ) -> IrResult<()>
    where
        'ctx: 'v;

    /// Collect cached references after [`Self::prefetch`]. The cache-miss branch
    /// is unreachable after a successful prefetch but reports
    /// [`IrError::AnalysisNotCached`] instead of panicking.
    fn collect<'v, 'r>(
        fam: &'r FunctionAnalysisManager<'ctx, B>,
        function: FunctionView<'v, B>,
    ) -> IrResult<Self::ResultRefs<'r>>
    where
        'ctx: 'v;
}

impl analysis_list_sealed::Sealed for () {}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionAnalysisList<'ctx, B> for () {
    const LEN: usize = 0;
    type ResultRefs<'r>
        = ()
    where
        'ctx: 'r;

    fn prefetch<'v>(
        _fam: &mut FunctionAnalysisManager<'ctx, B>,
        _function: FunctionView<'v, B>,
    ) -> IrResult<()>
    where
        'ctx: 'v,
    {
        Ok(())
    }

    fn collect<'v, 'r>(
        _fam: &'r FunctionAnalysisManager<'ctx, B>,
        _function: FunctionView<'v, B>,
    ) -> IrResult<Self::ResultRefs<'r>>
    where
        'ctx: 'v,
    {
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

            fn prefetch<'v>(
                fam: &mut FunctionAnalysisManager<'ctx, B>,
                function: FunctionView<'v, B>,
            ) -> IrResult<()>
            where
                'ctx: 'v,
            {
                $(
                    <$member as PrefetchableAnalysis<'ctx, B>>::ensure_registered(fam);
                    fam.result::<$member, _>(function)?;
                )+
                Ok(())
            }

            fn collect<'v, 'r>(
                fam: &'r FunctionAnalysisManager<'ctx, B>,
                function: FunctionView<'v, B>,
            ) -> IrResult<Self::ResultRefs<'r>>
            where
                'ctx: 'v,
            {
                Ok(($(
                    fam.cached_result::<$member, _>(function)
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

    /// Register (if needed) and compute every member for `module`. See
    /// [`FunctionAnalysisList::prefetch`] for why the view region `'v` is the
    /// driver's rather than the manager's.
    fn prefetch<'v>(
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        module: ModuleView<'v, B>,
    ) -> IrResult<()>
    where
        'ctx: 'v;

    /// Collect cached references after [`Self::prefetch`]. The cache-miss branch
    /// is unreachable after a successful prefetch but reports
    /// [`IrError::AnalysisNotCached`] instead of panicking.
    fn collect<'v, 'r>(
        mam: &'r ModuleAnalysisManager<'ctx, B>,
        module: ModuleView<'v, B>,
    ) -> IrResult<Self::ResultRefs<'r>>
    where
        'ctx: 'v;
}

impl<'ctx, B: ModuleBrand + 'ctx> ModuleAnalysisList<'ctx, B> for () {
    const LEN: usize = 0;
    type ResultRefs<'r>
        = ()
    where
        'ctx: 'r;

    fn prefetch<'v>(
        _mam: &mut ModuleAnalysisManager<'ctx, B>,
        _module: ModuleView<'v, B>,
    ) -> IrResult<()>
    where
        'ctx: 'v,
    {
        Ok(())
    }

    fn collect<'v, 'r>(
        _mam: &'r ModuleAnalysisManager<'ctx, B>,
        _module: ModuleView<'v, B>,
    ) -> IrResult<Self::ResultRefs<'r>>
    where
        'ctx: 'v,
    {
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

            fn prefetch<'v>(
                mam: &mut ModuleAnalysisManager<'ctx, B>,
                module: ModuleView<'v, B>,
            ) -> IrResult<()>
            where
                'ctx: 'v,
            {
                $(
                    mam.ensure_registered_default::<$member>();
                    mam.result_view::<$member>(module)?;
                )+
                Ok(())
            }

            fn collect<'v, 'r>(
                mam: &'r ModuleAnalysisManager<'ctx, B>,
                module: ModuleView<'v, B>,
            ) -> IrResult<Self::ResultRefs<'r>>
            where
                'ctx: 'v,
            {
                Ok(($(
                    mam.cached_result::<$member, _>(module)
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
    use crate::module::DynBrand;
    use crate::{Dyn, IrBuilder, Linkage};

    /// llvmkit-specific type-machinery lock (no upstream analog): the analysis-list
    /// tuple schema prefetches, collects, and selects by type. Runtime behavior it
    /// wraps (getResult caching) ports `unittests/IR/PassManagerTest.cpp`.
    #[test]
    fn analysis_list_prefetch_collect_select() -> IrResult<()> {
        let m = crate::module_new!("analysis-list")?;
        let i32_ty = m.i32_type();
        let fn_ty = m.function_type_no_parameters(i32_ty);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = m.view(f).append_basic_block(&m, "entry");
        let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        b.ret(i32_ty.const_int(0_u32))?;
        m.verify_borrowed()?;

        let function: FunctionView<'_, _> = m.view(f).into();
        let mut fam = FunctionAnalysisManager::new();
        type Reqs = (DominatorTreeAnalysis,);
        <Reqs as FunctionAnalysisList<'_, _>>::prefetch(&mut fam, function)?;
        let refs = <Reqs as FunctionAnalysisList<'_, _>>::collect(&fam, function)?;
        // `B` is pinned explicitly here: unlike `prefetch`/`collect`, `select`'s
        // only argument is `Self::ResultRefs<'r>`, whose concrete type
        // (`&DominatorTree`) doesn't mention `B`, so `_` has nothing to infer from.
        let dt: &DominatorTree =
            <Reqs as AnalysisSelector<'_, DynBrand, DominatorTreeAnalysis, Idx0>>::select(&refs);
        let entry_view = function
            .entry_block()
            .map(|bb| dt.is_reachable_from_entry(bb));
        assert_eq!(entry_view, Some(true));
        Ok(())
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
        let m = crate::module_new!("domtree-repair")?;
        let i32_ty = m.i32_type();
        let fn_ty = m.function_type_no_parameters(i32_ty);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = m.view(f).append_basic_block(&m, "entry");
        let next = m.view(f).append_basic_block(&m, "next");
        let entry_id = entry.slot();
        let next_id = next.slot();
        let next_label = next.id();

        // entry: br next    next: ret 0
        let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        b.br(next.id())?;
        let b2 = IrBuilder::new_for::<Dyn>(&m).position_at_end(next);
        b2.ret(i32_ty.const_int(0_u32))?;

        let function: FunctionView<'_, _> = m.view(f).into();

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
            CfgUpdate::insert(new_bb.slot(), next_id),
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
        fn run<'v>(
            &self,
            _function: FunctionView<'v, B>,
            _am: &mut FunctionAnalysisManager<'ctx, B>,
        ) -> IrResult<Self::Result>
        where
            'ctx: 'v,
        {
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
        let m = crate::module_new!("requires-no-default")?;
        let i32_ty = m.i32_type();
        let fn_ty = m.function_type_no_parameters(i32_ty);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = m.view(f).append_basic_block(&m, "entry");
        let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        b.ret(i32_ty.const_int(0_u32))?;

        let function: FunctionView<'_, _> = m.view(f).into();
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
            <Reqs as AnalysisSelector<'_, DynBrand, ThresholdAnalysis, Idx0>>::select(&refs);
        assert_eq!(result.threshold, 42);
        Ok(())
    }
}
