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
    FunctionAnalysisList, FunctionAnalysisManager, ModuleAnalysisList, ModuleAnalysisManager,
};
use crate::module::{Brand, Module, ModuleBrand, Unverified, Verified};
use crate::pass_access::{
    Downgrades, FnAccess, Inspect, ModAccess, PatchBody, PipelineVerdict, ReshapeCfg,
    RewriteModule, StaysVerified,
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
    fam: &mut FunctionAnalysisManager<'ctx, B>,
) -> IrResult<<<P::Access as FnAccess>::Verdict as PassExecution>::OutModule<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
    P: FunctionPass<'ctx, B>,
    P::Access: FnRungExecute,
    <P::Access as FnAccess>::Verdict: PassExecution,
    F: Into<FunctionView<'ctx, B>>,
{
    let function = function.into();
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
    mam: &mut ModuleAnalysisManager<'ctx, B>,
    fam: &mut FunctionAnalysisManager<'ctx, B>,
) -> IrResult<<<P::Access as ModAccess>::Verdict as PassExecution>::OutModule<'ctx, B>>
where
    B: ModuleBrand + 'ctx,
    P: ModulePass<'ctx, B>,
    P::Access: ModRungExecute,
    <P::Access as ModAccess>::Verdict: PassExecution,
{
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
