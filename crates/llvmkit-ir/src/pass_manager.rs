//! Single-pass driver for the capability-graded Pass API v2.
//!
//! A pass declares a capability *rung* ([`FnAccess`]/[`ModAccess`]) and its
//! required analyses; the driver derives everything else. This module supplies
//! the two public pass traits ([`FunctionPass`]/[`ModulePass`]), the
//! verdict→output-module mapping ([`PassExecution`]) shared with the pipeline
//! task, and the single-pass runners ([`run_function_pass`]/[`run_module_pass`])
//! that prefetch a pass's `Requires`, build its entry context, run it, and honor
//! the report's preservation set when invalidating the analysis managers.
//!
//! Scope: exactly one pass over one function / one module. Tuple pipelines,
//! instrumentation, and the erased `dyn` containers are separate tasks; the
//! [`PassExecution`] GAT here is written so the pipeline task can reuse it.
//!
//! The read-only path threads a verified module through untouched
//! ([`StaysVerified`] → `Module<Verified>`); a mutating rung downgrades the
//! module ([`Downgrades`] → `Module<Unverified>`), which must be re-verified
//! before the next verified-only stage (D8).

use crate::IrResult;
use crate::analysis::{
    AllAnalysesOnFunction, AllAnalysesOnModule, Analyses, FunctionAnalysisList,
    FunctionAnalysisManager, FunctionAnalysisManagerModuleProxy, ModuleAnalysisList,
    ModuleAnalysisManager, PreservedAnalyses,
};
use crate::module::{Brand, Module, ModuleBrand, ModuleView, Unverified, Verified};
use crate::pass_access::{
    Downgrades, FnAccess, Inspect, ModAccess, PatchBody, PipelineVerdict, ReshapeCfg,
    RewriteModule, StaysVerified, VerdictFold,
};
use crate::pass_context::{FnCx, FnReport, FunctionView, ModCx, ModReport};

/// A pass over one function body at capability rung [`Self::Access`].
///
/// The rung fixes how much the pass may mutate; the driver derives the verdict
/// (does the module stay verified?) and the preservation floor from it. The
/// `run` method takes its [`FnCx`] **by value**: the consuming transition into a
/// mutator is what makes over-claiming preservation unspellable (D1/D8).
pub trait FunctionPass<'ctx, B: ModuleBrand + 'ctx = Brand<'ctx>> {
    /// Capability rung: how much of the function body this pass may touch.
    type Access: FnAccess;
    /// Analyses prefetched before `run`; the context accessor is infallible.
    type Requires: FunctionAnalysisList<'ctx, B>;
    /// Instrumentation-facing name (unused by the bare driver; part of the API).
    const NAME: &'static str;
    /// Whether the pass must always run. Replaces the old runtime `is_required()`
    /// (only meaningful once instrumentation is wired in a later task).
    const REQUIRED: bool = false;

    /// Run the pass over one function, consuming its capability context.
    fn run(
        &mut self,
        cx: FnCx<'_, '_, 'ctx, B, Self::Access, Self::Requires>,
    ) -> IrResult<FnReport>;
}

/// A pass over one module at capability rung [`Self::Access`]. The module-level
/// mirror of [`FunctionPass`]. A module pass reaches per-function bodies by
/// calling `rewrite.for_each_function::<Rung>(...)` inline, so there is no
/// `FnAccess`/`FnRequires` associated type here — the function rung is chosen at
/// the call site.
pub trait ModulePass<'ctx, B: ModuleBrand + 'ctx = Brand<'ctx>> {
    /// Capability rung: how much of the module this pass may rewrite.
    type Access: ModAccess;
    /// Module analyses prefetched before `run`; the context accessor is infallible.
    type Requires: ModuleAnalysisList<'ctx, B>;
    /// Instrumentation-facing name (unused by the bare driver; part of the API).
    const NAME: &'static str;
    /// Whether the pass must always run. See [`FunctionPass::REQUIRED`].
    const REQUIRED: bool = false;

    /// Run the pass over one module, consuming its capability context.
    fn run(
        &mut self,
        cx: ModCx<'_, '_, '_, 'ctx, B, Self::Access, Self::Requires>,
    ) -> IrResult<ModReport>;
}

mod pass_execution_sealed {
    pub trait Sealed {}
    impl Sealed for super::StaysVerified {}
    impl Sealed for super::Downgrades {}
}

/// Maps a pass's derived [`PipelineVerdict`] to the module typestate a run
/// yields: [`StaysVerified`] keeps `Module<Verified>`; [`Downgrades`] hands back
/// `Module<Unverified>` (D8). Sealed to the two verdict markers.
///
/// This is the verdict→output-module seam mirrored from the old
/// `FunctionPipelineExecution`/`ModulePipelineExecution` but keyed on the
/// verdict rather than the old effect. The token construction lives on the
/// per-rung [`FnRungExecute`]/[`ModRungExecute`] traits below (only there is a
/// rung's `Token` concrete); this trait owns just the output-module GAT, which
/// the pipeline task reuses to spell a whole pipeline's return type.
pub trait PassExecution: PipelineVerdict + pass_execution_sealed::Sealed {
    /// Module typestate produced by a run whose derived verdict is `Self`.
    type OutModule<'ctx, B: ModuleBrand + 'ctx>;
}

impl PassExecution for StaysVerified {
    type OutModule<'ctx, B: ModuleBrand + 'ctx> = Module<'ctx, B, Verified>;
}

impl PassExecution for Downgrades {
    type OutModule<'ctx, B: ModuleBrand + 'ctx> = Module<'ctx, B, Unverified>;
}

mod fn_rung_sealed {
    pub trait Sealed {}
    impl Sealed for super::Inspect {}
    impl Sealed for super::PatchBody {}
    impl Sealed for super::ReshapeCfg {}
}

/// Per-rung execution seam for [`run_function_pass`]: builds the rung's entry
/// token, runs the pass, and returns the report plus the verdict-mapped module.
///
/// This lives on the rung (not on [`PassExecution`]) because only in a concrete
/// rung impl is [`FnAccess::Token`] a nameable type: the read-only rung's token
/// is `()` (no unverify), and the mutating rungs' token is the
/// `&Module<Unverified>` obtained from `module.unverify()`. A verdict-keyed
/// method could not construct that token generically. Sealed to the three
/// function rungs; hidden plumbing.
#[doc(hidden)]
pub trait FnRungExecute: FnAccess + fn_rung_sealed::Sealed {
    /// Run `pass` over `function` at this rung, given the prefetched `results`.
    fn execute<'ctx, B, R, P>(
        pass: &mut P,
        module: Module<'ctx, B, Verified>,
        function: FunctionView<'ctx, B>,
        results: R::ResultRefs<'_>,
    ) -> IrResult<(
        FnReport,
        <Self::Verdict as PassExecution>::OutModule<'ctx, B>,
    )>
    where
        B: ModuleBrand + 'ctx,
        R: FunctionAnalysisList<'ctx, B>,
        P: FunctionPass<'ctx, B, Access = Self, Requires = R>,
        Self::Verdict: PassExecution;
}

impl FnRungExecute for Inspect {
    fn execute<'ctx, B, R, P>(
        pass: &mut P,
        module: Module<'ctx, B, Verified>,
        function: FunctionView<'ctx, B>,
        results: R::ResultRefs<'_>,
    ) -> IrResult<(FnReport, Module<'ctx, B, Verified>)>
    where
        B: ModuleBrand + 'ctx,
        R: FunctionAnalysisList<'ctx, B>,
        P: FunctionPass<'ctx, B, Access = Inspect, Requires = R>,
    {
        // Read-only: no unverify, the token is `()`, the module flows out verified.
        let cx = FnCx::new((), function, results);
        let report = pass.run(cx)?;
        Ok((report, module))
    }
}

impl FnRungExecute for PatchBody {
    fn execute<'ctx, B, R, P>(
        pass: &mut P,
        module: Module<'ctx, B, Verified>,
        function: FunctionView<'ctx, B>,
        results: R::ResultRefs<'_>,
    ) -> IrResult<(FnReport, Module<'ctx, B, Unverified>)>
    where
        B: ModuleBrand + 'ctx,
        R: FunctionAnalysisList<'ctx, B>,
        P: FunctionPass<'ctx, B, Access = PatchBody, Requires = R>,
    {
        let unverified = module.unverify();
        let cx = FnCx::new(&unverified, function, results);
        let report = pass.run(cx)?;
        Ok((report, unverified))
    }
}

impl FnRungExecute for ReshapeCfg {
    fn execute<'ctx, B, R, P>(
        pass: &mut P,
        module: Module<'ctx, B, Verified>,
        function: FunctionView<'ctx, B>,
        results: R::ResultRefs<'_>,
    ) -> IrResult<(FnReport, Module<'ctx, B, Unverified>)>
    where
        B: ModuleBrand + 'ctx,
        R: FunctionAnalysisList<'ctx, B>,
        P: FunctionPass<'ctx, B, Access = ReshapeCfg, Requires = R>,
    {
        let unverified = module.unverify();
        let cx = FnCx::new(&unverified, function, results);
        let report = pass.run(cx)?;
        Ok((report, unverified))
    }
}

mod mod_rung_sealed {
    pub trait Sealed {}
    impl Sealed for super::Inspect {}
    impl Sealed for super::RewriteModule {}
}

/// Per-rung execution seam for [`run_module_pass`] — the module-level mirror of
/// [`FnRungExecute`]. Sealed to the two module rungs; hidden plumbing.
#[doc(hidden)]
pub trait ModRungExecute: ModAccess + mod_rung_sealed::Sealed {
    /// Run `pass` over `module` at this rung, given the prefetched `results`.
    fn execute<'ctx, 'r, B, R, P>(
        pass: &mut P,
        module: Module<'ctx, B, Verified>,
        mam: &'r ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
        results: R::ResultRefs<'r>,
    ) -> IrResult<(
        ModReport,
        <Self::Verdict as PassExecution>::OutModule<'ctx, B>,
    )>
    where
        B: ModuleBrand + 'ctx,
        R: ModuleAnalysisList<'ctx, B>,
        P: ModulePass<'ctx, B, Access = Self, Requires = R>,
        Self::Verdict: PassExecution;
}

impl ModRungExecute for Inspect {
    fn execute<'ctx, 'r, B, R, P>(
        pass: &mut P,
        module: Module<'ctx, B, Verified>,
        mam: &'r ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
        results: R::ResultRefs<'r>,
    ) -> IrResult<(ModReport, Module<'ctx, B, Verified>)>
    where
        B: ModuleBrand + 'ctx,
        R: ModuleAnalysisList<'ctx, B>,
        P: ModulePass<'ctx, B, Access = Inspect, Requires = R>,
    {
        let view = module.as_view();
        let cx = ModCx::new(view, (), results, mam, fam);
        let report = pass.run(cx)?;
        Ok((report, module))
    }
}

impl ModRungExecute for RewriteModule {
    fn execute<'ctx, 'r, B, R, P>(
        pass: &mut P,
        module: Module<'ctx, B, Verified>,
        mam: &'r ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
        results: R::ResultRefs<'r>,
    ) -> IrResult<(ModReport, Module<'ctx, B, Unverified>)>
    where
        B: ModuleBrand + 'ctx,
        R: ModuleAnalysisList<'ctx, B>,
        P: ModulePass<'ctx, B, Access = RewriteModule, Requires = R>,
    {
        let unverified = module.unverify();
        let view = unverified.as_view();
        let cx = ModCx::new(view, &unverified, results, mam, fam);
        let report = pass.run(cx)?;
        Ok((report, unverified))
    }
}

/// Run a single [`FunctionPass`] over one function of a verified module.
///
/// Prefetches the pass's `Requires`, builds its rung-specific entry context,
/// runs it, and invalidates `fam` with the report's preservation set. Returns
/// the verdict-mapped module: `Module<Verified>` for a read-only pass, the
/// downgraded `Module<Unverified>` for a mutating one (D8).
pub fn run_function_pass<'ctx, B, P, F>(
    mut pass: P,
    module: Module<'ctx, B, Verified>,
    function: F,
    analyses: &mut Analyses<'ctx, B>,
) -> IrResult<<<P::Access as FnAccess>::Verdict as PassExecution>::OutModule<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
    P: FunctionPass<'ctx, B>,
    P::Access: FnRungExecute,
    <P::Access as FnAccess>::Verdict: PassExecution,
    F: Into<FunctionView<'ctx, B>>,
{
    let function = function.into();
    let fam = analyses.function_manager_mut();
    P::Requires::prefetch(fam, function)?;
    let (report, out) = {
        // `results` borrows `*fam` only for this block; the returned report and
        // module borrow nothing from it, so `fam` is free for `invalidate`.
        let results = P::Requires::collect(&*fam, function)?;
        <P::Access as FnRungExecute>::execute::<B, P::Requires, P>(
            &mut pass, module, function, results,
        )?
    };
    fam.invalidate(function, &report.into_pa())?;
    Ok(out)
}

/// Run a single [`ModulePass`] over a verified module — the module-level mirror
/// of [`run_function_pass`]. Prefetches module `Requires`, runs the pass, and
/// invalidates both the module and function analysis managers with the report's
/// preservation set (mirroring the retired `ModulePassManager::run`).
pub fn run_module_pass<'ctx, B, P>(
    mut pass: P,
    module: Module<'ctx, B, Verified>,
    analyses: &mut Analyses<'ctx, B>,
) -> IrResult<<<P::Access as ModAccess>::Verdict as PassExecution>::OutModule<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
    P: ModulePass<'ctx, B>,
    P::Access: ModRungExecute,
    <P::Access as ModAccess>::Verdict: PassExecution,
{
    let (mam, fam) = analyses.managers_mut();
    let view = module.as_view();
    P::Requires::prefetch(mam, view)?;
    let (report, out) = {
        let results = P::Requires::collect(&*mam, view)?;
        <P::Access as ModRungExecute>::execute::<B, P::Requires, P>(
            &mut pass, module, &*mam, fam, results,
        )?
    };
    let pa = report.into_pa();
    mam.invalidate(view, &pa)?;
    fam.invalidate_module(view, &pa)?;
    Ok(out)
}

// ==========================================================================
// Pass API v2 — typed tuple pipelines (verdict-derived verification)
// ==========================================================================
//
// A pipeline composes several passes (and/or nested pipelines) and runs them in
// written order over one function / one module. Its output typestate is DERIVED
// from the members' capability rungs: the pipeline verdict is the [`VerdictFold`]
// of the members' [`FnAccess::Verdict`]/[`ModAccess::Verdict`] (all
// [`StaysVerified`] ⇒ `Module<Verified>`; any [`Downgrades`] ⇒
// `Module<Unverified>`), never a hand-written preservation claim (D1/D8). Ported
// from the retired effect-typed `FunctionPassList`/`ModulePassList` machinery,
// swapping the old effect/`ModuleToken`/`ProvidesToken`/`EffectFold` for the new
// verdict/[`VerdictCarry`]/[`ProvidesToken`]/[`VerdictFold`]. The public `run`
// entry points below take a single `&mut Analyses` bundling both managers;
// instrumentation is deliberately out of scope here (a later task).

/// The module capability a pipeline (or member) of verdict `Self` threads to its
/// members — the verdict-level mirror of the retired `TypedPassEffect::ModuleToken`.
/// [`StaysVerified`] carries nothing (`()`, all members are read-only);
/// [`Downgrades`] carries the shared `&Module<Unverified>` mutation token that
/// mutating members receive (built once by the pipeline's [`FunctionPipelineExecute`]
/// /[`ModulePipelineExecute`] when it downgrades the module). Sealed to the two
/// verdict markers through the [`PipelineVerdict`] supertrait.
pub trait VerdictCarry: PipelineVerdict {
    /// Carried mutation token; `()` for [`StaysVerified`],
    /// `&'pm Module<Unverified>` for [`Downgrades`].
    type Token<'pm, 'ctx, B: ModuleBrand + 'ctx>: Copy
    where
        'ctx: 'pm;
}

impl VerdictCarry for StaysVerified {
    type Token<'pm, 'ctx, B: ModuleBrand + 'ctx>
        = ()
    where
        'ctx: 'pm;
}

impl VerdictCarry for Downgrades {
    type Token<'pm, 'ctx, B: ModuleBrand + 'ctx>
        = &'pm Module<'ctx, B, Unverified>
    where
        'ctx: 'pm;
}

/// Weakens a pipeline verdict's carried token down to one member's verdict token
/// — the new analog of the retired `ProvidesToken`. A read-only member drops the
/// token (`_ -> ()`); a mutating member inside a downgraded pipeline receives it
/// unchanged (`&M -> &M`).
///
/// The missing `StaysVerified: ProvidesToken<Downgrades>` impl is intentional
/// (D1): a verified-staying pipeline cannot hand a mutation token to a mutating
/// member — but that pairing never arises, because a mutating member folds the
/// whole pipeline's verdict to [`Downgrades`] via [`VerdictFold`].
pub trait ProvidesToken<Member: VerdictCarry>: VerdictCarry {
    /// Project this pipeline verdict's token to `Member`'s token.
    fn member_token<'pm, 'ctx, B: ModuleBrand + 'ctx>(
        token: Self::Token<'pm, 'ctx, B>,
    ) -> Member::Token<'pm, 'ctx, B>
    where
        'ctx: 'pm;
}

impl ProvidesToken<StaysVerified> for StaysVerified {
    fn member_token<'pm, 'ctx, B: ModuleBrand + 'ctx>(_token: ())
    where
        'ctx: 'pm,
    {
    }
}

impl ProvidesToken<StaysVerified> for Downgrades {
    fn member_token<'pm, 'ctx, B: ModuleBrand + 'ctx>(_token: &'pm Module<'ctx, B, Unverified>)
    where
        'ctx: 'pm,
    {
    }
}

impl ProvidesToken<Downgrades> for Downgrades {
    fn member_token<'pm, 'ctx, B: ModuleBrand + 'ctx>(
        token: &'pm Module<'ctx, B, Unverified>,
    ) -> &'pm Module<'ctx, B, Unverified>
    where
        'ctx: 'pm,
    {
        token
    }
}

/// Type-level left-fold of a member-verdict cons list through [`VerdictFold`],
/// starting from [`StaysVerified`], to spell a pipeline's derived verdict. Folding
/// one pairwise join at a time (rather than a flat `where`-clause) is what makes
/// naming the folded type legal without the compiler first case-splitting the
/// sealed verdict set. Mirrors the retired `join_effects!`.
macro_rules! fold_verdicts {
    (@cons $only:ty) => { ($only, ()) };
    (@cons $head:ty, $($tail:ty),+) => {
        ($head, fold_verdicts!(@cons $($tail),+))
    };
    ($($verdict:ty),+) => {
        <fold_verdicts!(@cons $($verdict),+) as VerdictFold<StaysVerified>>::Out
    };
}

// -------------------------------------------------------------------------
// Function pipelines
// -------------------------------------------------------------------------

/// Prefetch `R`, build the member's [`FnCx`] at rung `A` from a pre-built `token`,
/// run `pass`, honor the report's preservation set when invalidating `fam`, and
/// return that set for the pipeline to intersect.
///
/// The shared per-member body factored from the pipeline runners: it is exactly
/// the prefetch/collect/invalidate flow of [`run_function_pass`] minus the
/// module-by-value verdict mapping (a pipeline threads a single pre-downgraded
/// token to all members instead of consuming a fresh `Module<Verified>` per
/// member). Because each member invalidates `fam` from its own report here, the
/// next member's prefetch already sees the fresh cache.
fn run_function_member<'pm, 'ctx, B, A, R, P>(
    pass: &mut P,
    token: A::Token<'pm, 'ctx, B>,
    function: FunctionView<'ctx, B>,
    fam: &mut FunctionAnalysisManager<'ctx, B>,
) -> IrResult<PreservedAnalyses>
where
    B: ModuleBrand + 'ctx,
    A: FnAccess,
    R: FunctionAnalysisList<'ctx, B>,
    P: FunctionPass<'ctx, B, Access = A, Requires = R>,
    'ctx: 'pm,
{
    R::prefetch(fam, function)?;
    let report = {
        // `results` borrows `*fam` only for this block; the returned report
        // borrows nothing from it, so `fam` is free for `invalidate`. The module
        // `token` is `Copy` and keeps its own longer `'pm`.
        let results = R::collect(&*fam, function)?;
        let cx = FnCx::new(token, function, results);
        pass.run(cx)?
    };
    let pa = report.into_pa();
    fam.invalidate(function, &pa)?;
    Ok(pa)
}

mod fn_member_sealed {
    pub trait Sealed {}
    impl Sealed for super::Inspect {}
    impl Sealed for super::PatchBody {}
    impl Sealed for super::ReshapeCfg {}
}

/// Per-rung member-execution seam for a function pipeline — the pipeline analog of
/// [`FnRungExecute`]. It exists for the same reason: only in a concrete rung impl
/// is the member's carried token (`<Self::Verdict as VerdictCarry>::Token`)
/// nameable as the concrete [`FnAccess::Token`] the [`FnCx`] wants, so a generic
/// leaf member cannot build its context without dispatching through the rung.
/// Sealed to the three function rungs; hidden plumbing.
#[doc(hidden)]
pub trait FnMemberExec: FnAccess + fn_member_sealed::Sealed {
    /// Run `pass` at this rung with the member's already-weakened `token`.
    fn run_member<'pm, 'ctx, B, R, P>(
        pass: &mut P,
        token: <Self::Verdict as VerdictCarry>::Token<'pm, 'ctx, B>,
        function: FunctionView<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<PreservedAnalyses>
    where
        B: ModuleBrand + 'ctx,
        R: FunctionAnalysisList<'ctx, B>,
        P: FunctionPass<'ctx, B, Access = Self, Requires = R>,
        Self::Verdict: VerdictCarry,
        'ctx: 'pm;
}

impl FnMemberExec for Inspect {
    fn run_member<'pm, 'ctx, B, R, P>(
        pass: &mut P,
        token: (),
        function: FunctionView<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<PreservedAnalyses>
    where
        B: ModuleBrand + 'ctx,
        R: FunctionAnalysisList<'ctx, B>,
        P: FunctionPass<'ctx, B, Access = Inspect, Requires = R>,
        'ctx: 'pm,
    {
        run_function_member::<B, Inspect, R, P>(pass, token, function, fam)
    }
}

impl FnMemberExec for PatchBody {
    fn run_member<'pm, 'ctx, B, R, P>(
        pass: &mut P,
        token: &'pm Module<'ctx, B, Unverified>,
        function: FunctionView<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<PreservedAnalyses>
    where
        B: ModuleBrand + 'ctx,
        R: FunctionAnalysisList<'ctx, B>,
        P: FunctionPass<'ctx, B, Access = PatchBody, Requires = R>,
        'ctx: 'pm,
    {
        run_function_member::<B, PatchBody, R, P>(pass, token, function, fam)
    }
}

impl FnMemberExec for ReshapeCfg {
    fn run_member<'pm, 'ctx, B, R, P>(
        pass: &mut P,
        token: &'pm Module<'ctx, B, Unverified>,
        function: FunctionView<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<PreservedAnalyses>
    where
        B: ModuleBrand + 'ctx,
        R: FunctionAnalysisList<'ctx, B>,
        P: FunctionPass<'ctx, B, Access = ReshapeCfg, Requires = R>,
        'ctx: 'pm,
    {
        run_function_member::<B, ReshapeCfg, R, P>(pass, token, function, fam)
    }
}

/// Dispatch tag distinguishing a leaf [`FunctionPass`] member from a nested
/// [`FunctionPipeline`] member. An inferred type parameter on
/// [`FunctionPipelineMember`]/[`ModulePipelineMember`] rather than an overlap: a
/// leaf pass implements the member trait only at [`LeafMember`], a pipeline only
/// at [`NestedMember`], so the blanket-vs-concrete pair never collides even
/// though nothing stops a downstream type from being both. Reused verbatim from
/// the retired machinery — the tag pattern is coherence-safe.
#[doc(hidden)]
pub struct LeafMember(());
/// Dispatch tag for a nested pipeline member. See [`LeafMember`].
#[doc(hidden)]
pub struct NestedMember(());

/// One member of a typed function pipeline: either a [`FunctionPass`] (via the
/// [`LeafMember`] impl) or a nested [`FunctionPipeline`] (via the
/// [`NestedMember`] impl). `Kind` is always inferred from the member type; do not
/// implement directly — the two provided impls are the whole intended universe.
pub trait FunctionPipelineMember<'ctx, B: ModuleBrand + 'ctx, Kind> {
    /// This member's contribution to the pipeline verdict.
    type MemberVerdict: VerdictCarry;

    #[doc(hidden)]
    fn run_member<'pm>(
        &mut self,
        token: <Self::MemberVerdict as VerdictCarry>::Token<'pm, 'ctx, B>,
        function: FunctionView<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<PreservedAnalyses>
    where
        'ctx: 'pm;
}

impl<'ctx, B, T> FunctionPipelineMember<'ctx, B, LeafMember> for T
where
    B: ModuleBrand + 'ctx,
    T: FunctionPass<'ctx, B>,
    T::Access: FnMemberExec,
    <T::Access as FnAccess>::Verdict: VerdictCarry,
{
    type MemberVerdict = <T::Access as FnAccess>::Verdict;

    fn run_member<'pm>(
        &mut self,
        token: <Self::MemberVerdict as VerdictCarry>::Token<'pm, 'ctx, B>,
        function: FunctionView<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<PreservedAnalyses>
    where
        'ctx: 'pm,
    {
        <T::Access as FnMemberExec>::run_member::<B, T::Requires, T>(self, token, function, fam)
    }
}

mod pass_list_sealed {
    pub trait Sealed {}
}

/// Tuple of function-pipeline members with a verdict derived by [`VerdictFold`].
/// Sealed — arities 1..=8, nest a [`FunctionPipeline`] as a member for longer
/// pipelines. `run_all` runs the members in written order; each member invalidates
/// `fam` from its own report (so member N+1 sees member N's invalidations), then
/// the aggregate is force-preserved over [`AllAnalysesOnFunction`] for the caller.
pub trait FunctionPassList<'ctx, B: ModuleBrand + 'ctx, Kinds>: pass_list_sealed::Sealed {
    /// Fold of every member's verdict: [`StaysVerified`] iff all members are.
    type Verdict: VerdictCarry;

    #[doc(hidden)]
    fn run_all<'pm>(
        &mut self,
        token: <Self::Verdict as VerdictCarry>::Token<'pm, 'ctx, B>,
        function: FunctionView<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<PreservedAnalyses>
    where
        'ctx: 'pm;
}

macro_rules! impl_function_pass_list {
    ($($member:ident . $kind:ident . $slot:tt),+) => {
        impl<$($member),+> pass_list_sealed::Sealed for ($($member,)+) {}

        impl<'ctx, B, $($member, $kind),+> FunctionPassList<'ctx, B, ($($kind,)+)>
            for ($($member,)+)
        where
            B: ModuleBrand + 'ctx,
            $($member: FunctionPipelineMember<'ctx, B, $kind>,)+
            fold_verdicts!($(<$member as FunctionPipelineMember<'ctx, B, $kind>>::MemberVerdict),+):
                $(ProvidesToken<
                    <$member as FunctionPipelineMember<'ctx, B, $kind>>::MemberVerdict,
                > +)+ VerdictCarry,
        {
            type Verdict =
                fold_verdicts!(
                    $(<$member as FunctionPipelineMember<'ctx, B, $kind>>::MemberVerdict),+
                );

            fn run_all<'pm>(
                &mut self,
                token: <Self::Verdict as VerdictCarry>::Token<'pm, 'ctx, B>,
                function: FunctionView<'ctx, B>,
                fam: &mut FunctionAnalysisManager<'ctx, B>,
            ) -> IrResult<PreservedAnalyses>
            where
                'ctx: 'pm,
            {
                let mut preserved = PreservedAnalyses::all();
                $(
                    let member_token = <Self::Verdict as ProvidesToken<
                        <$member as FunctionPipelineMember<'ctx, B, $kind>>::MemberVerdict,
                    >>::member_token(token);
                    let pa = FunctionPipelineMember::<'ctx, B, $kind>::run_member(
                        &mut self.$slot,
                        member_token,
                        function,
                        fam,
                    )?;
                    preserved.intersect(pa);
                )+
                preserved.preserve_set::<AllAnalysesOnFunction>();
                Ok(preserved)
            }
        }
    };
}

impl_function_pass_list!(P0.K0.0);
impl_function_pass_list!(P0.K0.0, P1.K1.1);
impl_function_pass_list!(P0.K0.0, P1.K1.1, P2.K2.2);
impl_function_pass_list!(P0.K0.0, P1.K1.1, P2.K2.2, P3.K3.3);
impl_function_pass_list!(P0.K0.0, P1.K1.1, P2.K2.2, P3.K3.3, P4.K4.4);
impl_function_pass_list!(P0.K0.0, P1.K1.1, P2.K2.2, P3.K3.3, P4.K4.4, P5.K5.5);
impl_function_pass_list!(
    P0.K0.0, P1.K1.1, P2.K2.2, P3.K3.3, P4.K4.4, P5.K5.5, P6.K6.6
);
impl_function_pass_list!(
    P0.K0.0, P1.K1.1, P2.K2.2, P3.K3.3, P4.K4.4, P5.K5.5, P6.K6.6, P7.K7.7
);

/// Statically-composed function pipeline. Built with [`function_pipeline`]; its
/// `run` output typestate (`Module<Verified>` vs `Module<Unverified>`) is derived
/// from the members' rungs via [`VerdictFold`], never declared (D8).
pub struct FunctionPipeline<P> {
    passes: P,
}

/// Compose a typed function pipeline from a tuple of [`FunctionPass`] impls and/or
/// nested [`FunctionPipeline`]s.
pub fn function_pipeline<P>(passes: P) -> FunctionPipeline<P> {
    FunctionPipeline { passes }
}

impl<P> FunctionPipeline<P> {
    /// Run over one function of a verified module. Returns `Module<Verified>` when
    /// every member's verdict folds to [`StaysVerified`], else the downgraded
    /// `Module<Unverified>` (D8). Each member runs on a fresh context built from
    /// the current `fam` state. `Kinds` is the inferred leaf/nested dispatch tuple.
    pub fn run<'ctx, B, F, Kinds>(
        &mut self,
        module: Module<'ctx, B, Verified>,
        function: F,
        analyses: &mut Analyses<'ctx, B>,
    ) -> IrResult<
        <<P as FunctionPassList<'ctx, B, Kinds>>::Verdict as PassExecution>::OutModule<'ctx, B>,
    >
    where
        B: ModuleBrand + 'ctx,
        P: FunctionPassList<'ctx, B, Kinds>,
        <P as FunctionPassList<'ctx, B, Kinds>>::Verdict: FunctionPipelineExecute,
        F: Into<FunctionView<'ctx, B>>,
    {
        <<P as FunctionPassList<'ctx, B, Kinds>>::Verdict as FunctionPipelineExecute>::execute::<
            B,
            P,
            Kinds,
        >(
            &mut self.passes,
            module,
            function.into(),
            analyses.function_manager_mut(),
        )
    }
}

/// Nested-pipeline member: a whole pipeline runs as one member of an outer list,
/// so the arity-8 cap never binds. Goes through [`FunctionPipelineMember`] at
/// [`NestedMember`] rather than the leaf [`FunctionPass`] blanket — the two never
/// overlap because [`FunctionPipeline`] does not implement [`FunctionPass`].
impl<'ctx, B, P, Kinds> FunctionPipelineMember<'ctx, B, (NestedMember, Kinds)>
    for FunctionPipeline<P>
where
    B: ModuleBrand + 'ctx,
    P: FunctionPassList<'ctx, B, Kinds>,
{
    type MemberVerdict = <P as FunctionPassList<'ctx, B, Kinds>>::Verdict;

    fn run_member<'pm>(
        &mut self,
        token: <Self::MemberVerdict as VerdictCarry>::Token<'pm, 'ctx, B>,
        function: FunctionView<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<PreservedAnalyses>
    where
        'ctx: 'pm,
    {
        self.passes.run_all(token, function, fam)
    }
}

/// Per-verdict `run` dispatch for a function pipeline: [`StaysVerified`] threads
/// the verified module through untouched (all members read-only, carry `()`);
/// [`Downgrades`] unverifies once up front and threads the shared
/// `&Module<Unverified>` to the members. Implemented by the two verdict markers.
pub trait FunctionPipelineExecute: VerdictCarry + PassExecution {
    #[doc(hidden)]
    fn execute<'ctx, B, P, Kinds>(
        passes: &mut P,
        module: Module<'ctx, B, Verified>,
        function: FunctionView<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<<Self as PassExecution>::OutModule<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
        P: FunctionPassList<'ctx, B, Kinds, Verdict = Self>;
}

impl FunctionPipelineExecute for StaysVerified {
    fn execute<'ctx, B, P, Kinds>(
        passes: &mut P,
        module: Module<'ctx, B, Verified>,
        function: FunctionView<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Module<'ctx, B, Verified>>
    where
        B: ModuleBrand + 'ctx,
        P: FunctionPassList<'ctx, B, Kinds, Verdict = Self>,
    {
        passes.run_all((), function, fam)?;
        Ok(module)
    }
}

impl FunctionPipelineExecute for Downgrades {
    fn execute<'ctx, B, P, Kinds>(
        passes: &mut P,
        module: Module<'ctx, B, Verified>,
        function: FunctionView<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Module<'ctx, B, Unverified>>
    where
        B: ModuleBrand + 'ctx,
        P: FunctionPassList<'ctx, B, Kinds, Verdict = Self>,
    {
        let unverified = module.unverify();
        passes.run_all(&unverified, function, fam)?;
        Ok(unverified)
    }
}

// -------------------------------------------------------------------------
// Module pipelines
// -------------------------------------------------------------------------

/// Prefetch `R`, build the member's [`ModCx`] at rung `A` from a pre-built
/// `token`, run `pass`, and return the report's preservation set. Unlike
/// [`run_function_member`], this does NOT invalidate: module-manager invalidation
/// is owned solely by the [`ModulePassList`] `run_all` loop, so leaf passes,
/// nested [`ModulePipeline`]s, and the [`ForEachFunction`] adaptor are all
/// invalidated from the same place (mirroring [`run_module_pass`]).
fn run_module_member<'pm, 'ctx, B, A, R, P>(
    pass: &mut P,
    token: A::Token<'pm, 'ctx, B>,
    module: ModuleView<'ctx, B>,
    mam: &mut ModuleAnalysisManager<'ctx, B>,
    fam: &mut FunctionAnalysisManager<'ctx, B>,
) -> IrResult<PreservedAnalyses>
where
    B: ModuleBrand + 'ctx,
    A: ModAccess,
    R: ModuleAnalysisList<'ctx, B>,
    P: ModulePass<'ctx, B, Access = A, Requires = R>,
    'ctx: 'pm,
{
    R::prefetch(mam, module)?;
    let report = {
        // `results` and the `&*mam` peek borrow `*mam` only for this block; `fam`
        // is reborrowed at the same scope. Both managers are free again for the
        // loop's `invalidate`/`invalidate_module`. The module `token` is `Copy`
        // and keeps its own longer `'pm`.
        let results = R::collect(&*mam, module)?;
        let cx = ModCx::new(module, token, results, &*mam, fam);
        pass.run(cx)?
    };
    Ok(report.into_pa())
}

mod mod_member_sealed {
    pub trait Sealed {}
    impl Sealed for super::Inspect {}
    impl Sealed for super::RewriteModule {}
}

/// Per-rung member-execution seam for a module pipeline — the module-level mirror
/// of [`FnMemberExec`]. Sealed to the two module rungs; hidden plumbing.
#[doc(hidden)]
pub trait ModMemberExec: ModAccess + mod_member_sealed::Sealed {
    /// Run `pass` at this rung with the member's already-weakened `token`.
    fn run_member<'pm, 'ctx, B, R, P>(
        pass: &mut P,
        token: <Self::Verdict as VerdictCarry>::Token<'pm, 'ctx, B>,
        module: ModuleView<'ctx, B>,
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<PreservedAnalyses>
    where
        B: ModuleBrand + 'ctx,
        R: ModuleAnalysisList<'ctx, B>,
        P: ModulePass<'ctx, B, Access = Self, Requires = R>,
        Self::Verdict: VerdictCarry,
        'ctx: 'pm;
}

impl ModMemberExec for Inspect {
    fn run_member<'pm, 'ctx, B, R, P>(
        pass: &mut P,
        token: (),
        module: ModuleView<'ctx, B>,
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<PreservedAnalyses>
    where
        B: ModuleBrand + 'ctx,
        R: ModuleAnalysisList<'ctx, B>,
        P: ModulePass<'ctx, B, Access = Inspect, Requires = R>,
        'ctx: 'pm,
    {
        run_module_member::<B, Inspect, R, P>(pass, token, module, mam, fam)
    }
}

impl ModMemberExec for RewriteModule {
    fn run_member<'pm, 'ctx, B, R, P>(
        pass: &mut P,
        token: &'pm Module<'ctx, B, Unverified>,
        module: ModuleView<'ctx, B>,
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<PreservedAnalyses>
    where
        B: ModuleBrand + 'ctx,
        R: ModuleAnalysisList<'ctx, B>,
        P: ModulePass<'ctx, B, Access = RewriteModule, Requires = R>,
        'ctx: 'pm,
    {
        run_module_member::<B, RewriteModule, R, P>(pass, token, module, mam, fam)
    }
}

/// One member of a typed module pipeline: a [`ModulePass`] (via [`LeafMember`]), a
/// nested [`ModulePipeline`] (via [`NestedMember`]), or a [`ForEachFunction`]
/// adaptor (via the `(Kinds,)` tag). `Kind` is always inferred; do not implement
/// directly. Mirrors [`FunctionPipelineMember`] at module scope.
pub trait ModulePipelineMember<'ctx, B: ModuleBrand + 'ctx, Kind> {
    /// This member's contribution to the pipeline verdict.
    type MemberVerdict: VerdictCarry;

    #[doc(hidden)]
    fn run_member<'pm>(
        &mut self,
        token: <Self::MemberVerdict as VerdictCarry>::Token<'pm, 'ctx, B>,
        module: ModuleView<'ctx, B>,
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<PreservedAnalyses>
    where
        'ctx: 'pm;
}

impl<'ctx, B, T> ModulePipelineMember<'ctx, B, LeafMember> for T
where
    B: ModuleBrand + 'ctx,
    T: ModulePass<'ctx, B>,
    T::Access: ModMemberExec,
    <T::Access as ModAccess>::Verdict: VerdictCarry,
{
    type MemberVerdict = <T::Access as ModAccess>::Verdict;

    fn run_member<'pm>(
        &mut self,
        token: <Self::MemberVerdict as VerdictCarry>::Token<'pm, 'ctx, B>,
        module: ModuleView<'ctx, B>,
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<PreservedAnalyses>
    where
        'ctx: 'pm,
    {
        <T::Access as ModMemberExec>::run_member::<B, T::Requires, T>(self, token, module, mam, fam)
    }
}

/// Runs a typed function pipeline over every function definition in module order
/// — the pipeline-adaptor analog of the retired `ModuleToFunctionPassAdaptor`
/// (`createModuleToFunctionPassAdaptor`, IR/PassManager.h). A module-pipeline
/// member; its verdict is the wrapped function pipeline's verdict (a mutating
/// function pipeline downgrades the module). Distinct from
/// [`crate::pass_context::ModRewrite::for_each_function`], which is a mutator
/// method for hand-written module passes.
pub struct ForEachFunction<P> {
    pipeline: FunctionPipeline<P>,
}

/// Wrap a function pipeline so it can run as one member of a [`ModulePipeline`],
/// visiting every function definition in module order.
pub fn for_each_function<P>(pipeline: FunctionPipeline<P>) -> ForEachFunction<P> {
    ForEachFunction { pipeline }
}

// The `Kind` slot is `(Kinds,)` — a 1-tuple wrapping the inner
// `FunctionPassList` dispatch tuple — rather than `Kinds` directly. That keeps
// this impl's `Kind` structurally distinct from the leaf `LeafMember` impl and
// the nested `(NestedMember, Kinds)` impl below, so coherence sees three
// non-overlapping shapes instead of a bare `Kinds` that could unify with either.
impl<'ctx, B, P, Kinds> ModulePipelineMember<'ctx, B, (Kinds,)> for ForEachFunction<P>
where
    B: ModuleBrand + 'ctx,
    P: FunctionPassList<'ctx, B, Kinds>,
{
    type MemberVerdict = <P as FunctionPassList<'ctx, B, Kinds>>::Verdict;

    fn run_member<'pm>(
        &mut self,
        token: <Self::MemberVerdict as VerdictCarry>::Token<'pm, 'ctx, B>,
        module: ModuleView<'ctx, B>,
        _mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<PreservedAnalyses>
    where
        'ctx: 'pm,
    {
        let mut preserved = PreservedAnalyses::all();
        for function in module.iter_functions() {
            // Skip declarations: only definitions have a body to run over.
            if function.entry_block().is_none() {
                continue;
            }
            let pa = self.pipeline.passes.run_all(token, function, fam)?;
            preserved.intersect(pa);
        }
        // The inner pipeline invalidated `fam` per function itself; force-preserve
        // all function analyses (and the FAM→module proxy) so the module loop's
        // `fam.invalidate_module` is a no-op and does not double-invalidate.
        preserved.preserve_set::<AllAnalysesOnFunction>();
        preserved.preserve::<FunctionAnalysisManagerModuleProxy>();
        Ok(preserved)
    }
}

mod module_pass_list_sealed {
    pub trait Sealed {}
}

/// Tuple of module-pipeline members with a verdict derived by [`VerdictFold`].
/// Sealed — arities 1..=8, nest a [`ModulePipeline`] as a member for longer
/// pipelines. `run_all` invalidates both the module and function analysis managers
/// after each member (mirroring [`run_module_pass`]), then force-preserves
/// [`AllAnalysesOnModule`] for the caller.
pub trait ModulePassList<'ctx, B: ModuleBrand + 'ctx, Kinds>:
    module_pass_list_sealed::Sealed
{
    /// Fold of every member's verdict: [`StaysVerified`] iff all members are.
    type Verdict: VerdictCarry;

    #[doc(hidden)]
    fn run_all<'pm>(
        &mut self,
        token: <Self::Verdict as VerdictCarry>::Token<'pm, 'ctx, B>,
        module: ModuleView<'ctx, B>,
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<PreservedAnalyses>
    where
        'ctx: 'pm;
}

macro_rules! impl_module_pass_list {
    ($($member:ident . $kind:ident . $slot:tt),+) => {
        impl<$($member),+> module_pass_list_sealed::Sealed for ($($member,)+) {}

        impl<'ctx, B, $($member, $kind),+> ModulePassList<'ctx, B, ($($kind,)+)>
            for ($($member,)+)
        where
            B: ModuleBrand + 'ctx,
            $($member: ModulePipelineMember<'ctx, B, $kind>,)+
            fold_verdicts!($(<$member as ModulePipelineMember<'ctx, B, $kind>>::MemberVerdict),+):
                $(ProvidesToken<
                    <$member as ModulePipelineMember<'ctx, B, $kind>>::MemberVerdict,
                > +)+ VerdictCarry,
        {
            type Verdict =
                fold_verdicts!(
                    $(<$member as ModulePipelineMember<'ctx, B, $kind>>::MemberVerdict),+
                );

            fn run_all<'pm>(
                &mut self,
                token: <Self::Verdict as VerdictCarry>::Token<'pm, 'ctx, B>,
                module: ModuleView<'ctx, B>,
                mam: &mut ModuleAnalysisManager<'ctx, B>,
                fam: &mut FunctionAnalysisManager<'ctx, B>,
            ) -> IrResult<PreservedAnalyses>
            where
                'ctx: 'pm,
            {
                let mut preserved = PreservedAnalyses::all();
                $(
                    let member_token = <Self::Verdict as ProvidesToken<
                        <$member as ModulePipelineMember<'ctx, B, $kind>>::MemberVerdict,
                    >>::member_token(token);
                    let pa = ModulePipelineMember::<'ctx, B, $kind>::run_member(
                        &mut self.$slot,
                        member_token,
                        module,
                        mam,
                        fam,
                    )?;
                    mam.invalidate(module, &pa)?;
                    fam.invalidate_module(module, &pa)?;
                    preserved.intersect(pa);
                )+
                preserved.preserve_set::<AllAnalysesOnModule>();
                Ok(preserved)
            }
        }
    };
}

impl_module_pass_list!(P0.K0.0);
impl_module_pass_list!(P0.K0.0, P1.K1.1);
impl_module_pass_list!(P0.K0.0, P1.K1.1, P2.K2.2);
impl_module_pass_list!(P0.K0.0, P1.K1.1, P2.K2.2, P3.K3.3);
impl_module_pass_list!(P0.K0.0, P1.K1.1, P2.K2.2, P3.K3.3, P4.K4.4);
impl_module_pass_list!(P0.K0.0, P1.K1.1, P2.K2.2, P3.K3.3, P4.K4.4, P5.K5.5);
impl_module_pass_list!(
    P0.K0.0, P1.K1.1, P2.K2.2, P3.K3.3, P4.K4.4, P5.K5.5, P6.K6.6
);
impl_module_pass_list!(
    P0.K0.0, P1.K1.1, P2.K2.2, P3.K3.3, P4.K4.4, P5.K5.5, P6.K6.6, P7.K7.7
);

/// Statically-composed module pipeline. Built with [`module_pipeline`]; its `run`
/// output typestate is derived from the members' rungs via [`VerdictFold`], never
/// declared (D8). Mirrors [`FunctionPipeline`] at module scope.
pub struct ModulePipeline<P> {
    passes: P,
}

/// Compose a typed module pipeline from a tuple of [`ModulePass`] impls, nested
/// [`ModulePipeline`]s, and/or [`ForEachFunction`] adaptors.
pub fn module_pipeline<P>(passes: P) -> ModulePipeline<P> {
    ModulePipeline { passes }
}

impl<P> ModulePipeline<P> {
    /// Run over a verified module. Returns `Module<Verified>` when every member's
    /// verdict folds to [`StaysVerified`], else the downgraded `Module<Unverified>`
    /// (D8). Invalidates both managers after each member. `Kinds` is the inferred
    /// leaf/nested/for-each dispatch tuple.
    pub fn run<'ctx, B, Kinds>(
        &mut self,
        module: Module<'ctx, B, Verified>,
        analyses: &mut Analyses<'ctx, B>,
    ) -> IrResult<
        <<P as ModulePassList<'ctx, B, Kinds>>::Verdict as PassExecution>::OutModule<'ctx, B>,
    >
    where
        B: ModuleBrand + 'ctx,
        P: ModulePassList<'ctx, B, Kinds>,
        <P as ModulePassList<'ctx, B, Kinds>>::Verdict: ModulePipelineExecute,
    {
        let (mam, fam) = analyses.managers_mut();
        <<P as ModulePassList<'ctx, B, Kinds>>::Verdict as ModulePipelineExecute>::execute::<
            B,
            P,
            Kinds,
        >(&mut self.passes, module, mam, fam)
    }
}

/// Nested-pipeline member: a whole pipeline runs as one member of an outer list.
/// Mirrors the [`FunctionPipelineMember`] nesting impl at module scope — the two
/// never overlap because [`ModulePipeline`] does not implement [`ModulePass`].
impl<'ctx, B, P, Kinds> ModulePipelineMember<'ctx, B, (NestedMember, Kinds)> for ModulePipeline<P>
where
    B: ModuleBrand + 'ctx,
    P: ModulePassList<'ctx, B, Kinds>,
{
    type MemberVerdict = <P as ModulePassList<'ctx, B, Kinds>>::Verdict;

    fn run_member<'pm>(
        &mut self,
        token: <Self::MemberVerdict as VerdictCarry>::Token<'pm, 'ctx, B>,
        module: ModuleView<'ctx, B>,
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<PreservedAnalyses>
    where
        'ctx: 'pm,
    {
        self.passes.run_all(token, module, mam, fam)
    }
}

/// Per-verdict `run` dispatch for a module pipeline — the module-level mirror of
/// [`FunctionPipelineExecute`]. Implemented by the two verdict markers.
pub trait ModulePipelineExecute: VerdictCarry + PassExecution {
    #[doc(hidden)]
    fn execute<'ctx, B, P, Kinds>(
        passes: &mut P,
        module: Module<'ctx, B, Verified>,
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<<Self as PassExecution>::OutModule<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
        P: ModulePassList<'ctx, B, Kinds, Verdict = Self>;
}

impl ModulePipelineExecute for StaysVerified {
    fn execute<'ctx, B, P, Kinds>(
        passes: &mut P,
        module: Module<'ctx, B, Verified>,
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Module<'ctx, B, Verified>>
    where
        B: ModuleBrand + 'ctx,
        P: ModulePassList<'ctx, B, Kinds, Verdict = Self>,
    {
        let view = module.as_view();
        passes.run_all((), view, mam, fam)?;
        Ok(module)
    }
}

impl ModulePipelineExecute for Downgrades {
    fn execute<'ctx, B, P, Kinds>(
        passes: &mut P,
        module: Module<'ctx, B, Verified>,
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Module<'ctx, B, Unverified>>
    where
        B: ModuleBrand + 'ctx,
        P: ModulePassList<'ctx, B, Kinds, Verdict = Self>,
    {
        let unverified = module.unverify();
        let view = unverified.as_view();
        passes.run_all(&unverified, view, mam, fam)?;
        Ok(unverified)
    }
}
