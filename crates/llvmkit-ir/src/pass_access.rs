//! Capability-rung lattice foundations for the Pass API v2.
//!
//! A pass declares a *capability rung* — how much of the IR it may mutate — and
//! the driver *derives* the rest: whether the run keeps the module verified and
//! which analyses survive. Author intent flows one way (rung in), consequences
//! flow the other (verdict + preservation floor out); a pass can never claim to
//! preserve more than its rung structurally allows. This is the type-level
//! backbone that makes "mutated the IR but declared everything preserved"
//! unrepresentable rather than merely discouraged (D1: make illegal states
//! unrepresentable; D8: verified-flow — a mutating rung downgrades the pipeline
//! verdict so the module must be re-verified before the next verified-only
//! stage).
//!
//! The rungs form a read/mutate hierarchy split across two capability traits:
//!
//! - [`FnAccess`] — capabilities usable over a single function body:
//!   [`Inspect`] (read-only), [`PatchBody`] (edit instructions, no CFG change),
//!   [`ReshapeCfg`] (rewire the CFG, add/remove/split blocks, new PHIs).
//! - [`ModAccess`] — capabilities usable over a whole module: [`Inspect`]
//!   (read-only) and [`RewriteModule`] (globals, functions, ctors, bodies).
//!
//! [`Inspect`] is the only rung valid at *both* levels, and the only one whose
//! verdict is [`StaysVerified`]; every mutating rung derives [`Downgrades`].
//! A pipeline's overall verdict is the type-level join of its members' verdicts
//! (see [`PipelineVerdict`] and [`VerdictFold`]): read-only is the identity,
//! any mutating member is absorbing.

use crate::analysis::{CFGAnalyses, FunctionAnalysisList, PreservedAnalyses};
use crate::module::{Module, ModuleBrand, Unverified};
use crate::pass_context::FunctionView;

mod access_sealed {
    pub trait Sealed {}
}

/// Read-only rung. Valid at both function and module level; the only rung whose
/// [`PipelineVerdict`] is [`StaysVerified`].
pub enum Inspect {}

/// Function rung: edit instructions within existing blocks. No terminator or
/// control-flow-graph change, so CFG-shaped analyses survive.
pub enum PatchBody {}

/// Function rung: rewire branches, add/remove/split blocks, introduce new PHIs.
/// Reshapes the CFG, so nothing is preserved by default.
pub enum ReshapeCfg {}

/// Module rung: rewrite globals, functions, constructors, and per-function
/// bodies. Nothing is preserved by default.
pub enum RewriteModule {}

impl access_sealed::Sealed for Inspect {}
impl access_sealed::Sealed for PatchBody {}
impl access_sealed::Sealed for ReshapeCfg {}
impl access_sealed::Sealed for RewriteModule {}

mod verdict_sealed {
    pub trait Sealed {}
}

/// A pipeline's contribution to whether the module stays verified. Sealed to the
/// two members below; the join is the two-point lattice with [`StaysVerified`]
/// as identity and [`Downgrades`] as the absorbing element.
pub trait PipelineVerdict: verdict_sealed::Sealed + 'static {
    /// `Self ⊔ Rhs`. Total so a fold over abstract members type-checks without
    /// the compiler case-splitting the sealed set. [`StaysVerified`] is the
    /// identity; [`Downgrades`] is absorbing.
    type JoinWith<Rhs: PipelineVerdict>: PipelineVerdict;
}

/// Verdict of an all-read-only pipeline: the module stays `Module<Verified>`.
pub enum StaysVerified {}

/// Verdict once any member mutates: the module becomes `Module<Unverified>` and
/// must be re-verified before the next verified-only stage (D8).
pub enum Downgrades {}

impl verdict_sealed::Sealed for StaysVerified {}
impl verdict_sealed::Sealed for Downgrades {}

impl PipelineVerdict for StaysVerified {
    type JoinWith<Rhs: PipelineVerdict> = Rhs;
}

impl PipelineVerdict for Downgrades {
    type JoinWith<Rhs: PipelineVerdict> = Downgrades;
}

/// Type-level left-fold of a cons list of [`PipelineVerdict`] markers through
/// [`PipelineVerdict::JoinWith`], threading a running accumulator `Acc`. `()`
/// yields the accumulator; `(Head, Tail)` joins `Head` onto `Acc` and recurses.
/// Because `JoinWith` is total over an arbitrary `Rhs`, each step is well-formed
/// for abstract member verdicts without the compiler having to case-split the
/// sealed set — a flat `where`-clause fold cannot express this. The pipeline
/// task feeds this a members' cons-list to spell a pipeline's derived verdict.
pub trait VerdictFold<Acc: PipelineVerdict> {
    /// The joined verdict of the cons list, starting from `Acc`.
    type Out: PipelineVerdict;
}

impl<Acc: PipelineVerdict> VerdictFold<Acc> for () {
    type Out = Acc;
}

impl<Acc, Head, Tail> VerdictFold<Acc> for (Head, Tail)
where
    Acc: PipelineVerdict,
    Head: PipelineVerdict,
    Tail: VerdictFold<Head::JoinWith<Acc>>,
{
    type Out = <Tail as VerdictFold<Head::JoinWith<Acc>>>::Out;
}

/// Capability rung usable over a single function body. Sealed: only the four
/// rung ZSTs in this module implement it.
pub trait FnAccess: access_sealed::Sealed + 'static {
    /// Does holding this rung permit ANY mutation? (Convenience/const-eval; the
    /// pipeline verdict uses [`Self::Verdict`], not this.)
    const MUTATES: bool;
    /// Type-level contribution to a pipeline's verified/unverified verdict.
    type Verdict: PipelineVerdict;
    /// The module capability this rung's context holds. `()` for read-only
    /// ([`Inspect`]) — no unverify needed, the module stays `Verified`;
    /// `&Module<Unverified>` for the mutating rungs — the interior-mutability
    /// mutation token. Mirrors the existing
    /// [`crate::pass_manager::TypedPassEffect::ModuleToken`].
    type Token<'pm, 'ctx, B: ModuleBrand + 'ctx>: Copy
    where
        'ctx: 'pm;
    /// Preservation floor the driver applies after a mutating run at this rung.
    /// DERIVED, never author-supplied; always a SAFE under-approximation.
    #[doc(hidden)]
    fn preserved_floor() -> PreservedAnalyses;
}

/// Capability rung usable over a whole module. Sealed: only the rung ZSTs in
/// this module implement it.
pub trait ModAccess: access_sealed::Sealed + 'static {
    /// Does holding this rung permit ANY mutation? (Convenience/const-eval; the
    /// pipeline verdict uses [`Self::Verdict`], not this.)
    const MUTATES: bool;
    /// Type-level contribution to a pipeline's verified/unverified verdict.
    type Verdict: PipelineVerdict;
    /// The module capability this rung's context holds. `()` for read-only
    /// ([`Inspect`]); `&Module<Unverified>` for [`RewriteModule`]. Mirrors the
    /// existing [`crate::pass_manager::TypedPassEffect::ModuleToken`]; consumed
    /// by Task 2b's module-level context.
    type Token<'pm, 'ctx, B: ModuleBrand + 'ctx>: Copy
    where
        'ctx: 'pm;
    /// Preservation floor the driver applies after a mutating run at this rung.
    /// DERIVED, never author-supplied; always a SAFE under-approximation.
    #[doc(hidden)]
    fn preserved_floor() -> PreservedAnalyses;
}

impl FnAccess for Inspect {
    const MUTATES: bool = false;
    type Verdict = StaysVerified;
    type Token<'pm, 'ctx, B: ModuleBrand + 'ctx>
        = ()
    where
        'ctx: 'pm;

    fn preserved_floor() -> PreservedAnalyses {
        PreservedAnalyses::all()
    }
}

impl ModAccess for Inspect {
    const MUTATES: bool = false;
    type Verdict = StaysVerified;
    type Token<'pm, 'ctx, B: ModuleBrand + 'ctx>
        = ()
    where
        'ctx: 'pm;

    fn preserved_floor() -> PreservedAnalyses {
        PreservedAnalyses::all()
    }
}

impl FnAccess for PatchBody {
    const MUTATES: bool = true;
    type Verdict = Downgrades;
    type Token<'pm, 'ctx, B: ModuleBrand + 'ctx>
        = &'pm Module<'ctx, B, Unverified>
    where
        'ctx: 'pm;

    fn preserved_floor() -> PreservedAnalyses {
        PreservedAnalyses::all_in_set::<CFGAnalyses>()
    }
}

impl FnAccess for ReshapeCfg {
    const MUTATES: bool = true;
    type Verdict = Downgrades;
    type Token<'pm, 'ctx, B: ModuleBrand + 'ctx>
        = &'pm Module<'ctx, B, Unverified>
    where
        'ctx: 'pm;

    fn preserved_floor() -> PreservedAnalyses {
        PreservedAnalyses::none()
    }
}

impl ModAccess for RewriteModule {
    const MUTATES: bool = true;
    type Verdict = Downgrades;
    type Token<'pm, 'ctx, B: ModuleBrand + 'ctx>
        = &'pm Module<'ctx, B, Unverified>
    where
        'ctx: 'pm;

    fn preserved_floor() -> PreservedAnalyses {
        PreservedAnalyses::none()
    }
}

/// A [`FnAccess`] rung that permits mutation. Deferred out of Task 1 so it can
/// name the mutator types built in Task 2a. Implemented for [`PatchBody`] and
/// [`ReshapeCfg`] only — [`Inspect`] deliberately has no impl, which is exactly
/// what removes `mutate()`/`unchanged()` from a read-only context (read-only is
/// structural, not checked; D1). Sealed through the [`FnAccess`] supertrait.
///
/// The mutator itself (`FnPatch`/`FnReshape`) carries the mutation token and the
/// prefetched analysis results, so a transform can read analyses *while* it
/// edits; see [`crate::pass_context`].
pub trait MutatingFn: FnAccess {
    /// The rung-specific mutator [`crate::pass_context::FnCx::mutate`] hands out
    /// once it has consumed the entry context. `'m` borrows the module token,
    /// `'r` borrows the prefetched results (mirrors the context's two-lifetime
    /// split).
    type Mutator<'m, 'r, 'ctx, B: ModuleBrand + 'ctx, R: FunctionAnalysisList<'ctx, B>>
    where
        'ctx: 'm,
        'ctx: 'r;

    /// Build the mutator from the consumed context's parts. Internal plumbing
    /// for [`crate::pass_context::FnCx::mutate`]; hidden from authors (the rung
    /// impls live next to the mutator definitions in `pass_context`).
    #[doc(hidden)]
    fn into_mutator<'m, 'r, 'ctx, B, R>(
        token: Self::Token<'m, 'ctx, B>,
        function: FunctionView<'ctx, B>,
        results: R::ResultRefs<'r>,
    ) -> Self::Mutator<'m, 'r, 'ctx, B, R>
    where
        B: ModuleBrand + 'ctx,
        R: FunctionAnalysisList<'ctx, B>,
        'ctx: 'm,
        'ctx: 'r;
}

#[cfg(test)]
mod tests {
    use super::{
        Downgrades, FnAccess, Inspect, ModAccess, PatchBody, PipelineVerdict, ReshapeCfg,
        RewriteModule, StaysVerified, VerdictFold,
    };
    use crate::DominatorTreeAnalysis;
    use crate::analysis::CFGAnalyses;

    /// llvmkit-specific capability-lattice lock (no upstream analog: LLVM has no
    /// compile-time pass-capability/verdict distinction).
    #[test]
    fn preserved_floor_values() {
        // `Inspect` reads only; both its function and module floors keep every
        // analysis.
        assert!(<Inspect as FnAccess>::preserved_floor().are_all_preserved());
        assert!(<Inspect as ModAccess>::preserved_floor().are_all_preserved());

        // `PatchBody` edits within blocks: CFG-shaped analyses survive, but a
        // concrete non-CFG analysis is not individually preserved.
        let patch = <PatchBody as FnAccess>::preserved_floor();
        let patch_checker = patch.checker::<DominatorTreeAnalysis>();
        assert!(patch_checker.preserved_set::<CFGAnalyses>());
        assert!(!patch_checker.preserved());

        // `ReshapeCfg` rewires control flow: nothing is preserved, not even the
        // CFG set, for an arbitrary analysis.
        let reshape = <ReshapeCfg as FnAccess>::preserved_floor();
        let reshape_checker = reshape.checker::<DominatorTreeAnalysis>();
        assert!(!reshape_checker.preserved());
        assert!(!reshape_checker.preserved_set::<CFGAnalyses>());

        // `RewriteModule` rewrites the module: nothing is preserved.
        let rewrite = <RewriteModule as ModAccess>::preserved_floor();
        let rewrite_checker = rewrite.checker::<DominatorTreeAnalysis>();
        assert!(!rewrite_checker.preserved());
        assert!(!rewrite_checker.preserved_set::<CFGAnalyses>());

        // `MUTATES` mirrors each rung: read-only for `Inspect` (at both levels),
        // mutating for the three transform rungs. Bound through a local so the
        // check reads the associated consts rather than folding to a literal.
        let mutates = (
            <Inspect as FnAccess>::MUTATES,
            <Inspect as ModAccess>::MUTATES,
            <PatchBody as FnAccess>::MUTATES,
            <ReshapeCfg as FnAccess>::MUTATES,
            <RewriteModule as ModAccess>::MUTATES,
        );
        assert_eq!(mutates, (false, false, true, true, true));
    }

    /// llvmkit-specific capability-lattice lock (no upstream analog: LLVM has no
    /// compile-time pass-capability/verdict distinction).
    #[test]
    fn verdict_join_truth_table() {
        fn stays<A, B>()
        where
            A: PipelineVerdict<JoinWith<B> = StaysVerified>,
            B: PipelineVerdict,
        {
        }
        fn downgrades<A, B>()
        where
            A: PipelineVerdict<JoinWith<B> = Downgrades>,
            B: PipelineVerdict,
        {
        }

        // Read-only identity; mutating absorbing.
        stays::<StaysVerified, StaysVerified>();
        downgrades::<StaysVerified, Downgrades>();
        downgrades::<Downgrades, StaysVerified>();
        downgrades::<Downgrades, Downgrades>();
    }

    /// llvmkit-specific capability-lattice lock (no upstream analog: LLVM has no
    /// compile-time pass-capability/verdict distinction).
    #[test]
    fn verdict_fold_over_members() {
        fn assert_fold<L, Out>()
        where
            L: VerdictFold<StaysVerified, Out = Out>,
        {
        }

        // Empty pipeline stays at the identity.
        assert_fold::<(), StaysVerified>();
        // Every member read-only => the pipeline stays verified.
        assert_fold::<(StaysVerified, (StaysVerified, ())), StaysVerified>();
        // A single mutating member anywhere downgrades the whole pipeline.
        assert_fold::<(StaysVerified, (Downgrades, ())), Downgrades>();
        assert_fold::<(Downgrades, (StaysVerified, ())), Downgrades>();
        assert_fold::<(Downgrades, (Downgrades, ())), Downgrades>();
    }

    /// llvmkit-specific capability-lattice lock (no upstream analog: LLVM has no
    /// compile-time pass-capability/verdict distinction).
    #[test]
    fn lattice_membership() {
        fn require_fn<A: FnAccess>() {}
        fn require_mod<A: ModAccess>() {}
        fn require_fn_verdict<A, V>()
        where
            A: FnAccess<Verdict = V>,
            V: PipelineVerdict,
        {
        }
        fn require_mod_verdict<A, V>()
        where
            A: ModAccess<Verdict = V>,
            V: PipelineVerdict,
        {
        }

        // `Inspect` is the only rung valid at both levels.
        require_fn::<Inspect>();
        require_mod::<Inspect>();
        require_fn::<PatchBody>();
        require_fn::<ReshapeCfg>();
        require_mod::<RewriteModule>();

        // Each rung derives the expected verdict.
        require_fn_verdict::<Inspect, StaysVerified>();
        require_mod_verdict::<Inspect, StaysVerified>();
        require_fn_verdict::<PatchBody, Downgrades>();
        require_fn_verdict::<ReshapeCfg, Downgrades>();
        require_mod_verdict::<RewriteModule, Downgrades>();
    }
}
