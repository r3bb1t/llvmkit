//! Minimal module/function pass managers. This mirrors LLVM's new-PM
//! sequencing and invalidation shape without textual pipelines, loop PM,
//! CGSCC, or transform libraries. Read-only managers preserve verification at
//! the type level; transform managers receive mutation capability and therefore
//! always return an unverified module that must be verified before the next
//! verified-only pipeline.

use core::marker::PhantomData;

use super::pass_pipeline::{FunctionPassScope, ModulePassScope, PassName, PassScope};
use crate::IrResult;
use crate::analysis::{
    AllAnalysesOnFunction, AllAnalysesOnModule, FunctionAnalysisList, FunctionAnalysisManager,
    FunctionAnalysisManagerModuleProxy, ModuleAnalysisList, ModuleAnalysisManager,
    PreservationBound, PreservedAnalyses,
};
use crate::module::{Brand, Module, ModuleBrand, ModuleView, Unverified, Verified};
use crate::pass_context::{
    FunctionPassContext, FunctionView, ModulePassContext, ReadOnlyFunctionPassContext,
    ReadOnlyModulePassContext, TypedFunctionPassContext, TypedModulePassContext,
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

/// Static typed pipeline metadata for future in-tree pass structs.
pub trait PassPipelineInfo {
    /// Pass-manager scope where this pass may be inserted by pipeline name.
    type Scope: PassScope;

    /// Canonical typed pipeline name.
    const PIPELINE_NAME: PassName<Self::Scope>;
}

trait ErasedFunctionPass<'ctx, B: ModuleBrand + 'ctx, E: ModulePassEffect> {
    fn run(&mut self, cx: &mut E::FunctionContext<'_, 'ctx, B>) -> IrResult<PreservedAnalyses>;

    fn name(&self) -> &str;
    fn is_required(&self) -> bool;
}

struct FunctionPassModel<P> {
    name: String,
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

    fn name(&self) -> &str {
        &self.name
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

    fn name(&self) -> &str {
        &self.name
    }

    fn is_required(&self) -> bool {
        self.pass.is_required()
    }
}

trait ErasedModulePass<'ctx, B: ModuleBrand + 'ctx, E: ModulePassEffect> {
    fn run(&mut self, cx: &mut E::ModuleContext<'_, 'ctx, B>) -> IrResult<PreservedAnalyses>;

    fn name(&self) -> &str;
    fn is_required(&self) -> bool;
}

struct ModulePassModel<P> {
    name: String,
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

    fn name(&self) -> &str {
        &self.name
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

    fn name(&self) -> &str {
        &self.name
    }

    fn is_required(&self) -> bool {
        self.pass.is_required()
    }
}

/// Effect extension for the typed pipeline path: the module capability a pass
/// of this effect receives. Read-only passes get no token; transform passes
/// get the unverified-module mutation token (D8).
pub trait TypedPassEffect: ModulePassEffect + JoinsAll {
    type ModuleToken<'pm, 'ctx, B: ModuleBrand + 'ctx>: Copy
    where
        'ctx: 'pm;
}

/// Totality witness for the effect join: every effect joins with *every* other
/// effect, expressed as a generic associated type so a bound `E: TypedPassEffect`
/// implies `E ⊔ Rhs` is nameable for an arbitrary `Rhs` — including the abstract
/// running accumulator of [`EffectFold`]. Without this, folding a pipeline's
/// effect over abstract member effects would force the compiler to case-split
/// the sealed effect set, which it will not do. Mirrors the same
/// identity/absorbing lattice as [`EffectJoin`].
pub trait JoinsAll {
    /// `Self ⊔ Rhs`.
    type JoinOut<Rhs: TypedPassEffect>: TypedPassEffect;
}

/// Read-only is the lattice identity: `PreservesVerification ⊔ Rhs = Rhs`.
impl JoinsAll for PreservesVerification {
    type JoinOut<Rhs: TypedPassEffect> = Rhs;
}

/// Transform is absorbing: `MutatesIr ⊔ Rhs = MutatesIr`.
impl JoinsAll for MutatesIr {
    type JoinOut<Rhs: TypedPassEffect> = MutatesIr;
}

impl TypedPassEffect for PreservesVerification {
    type ModuleToken<'pm, 'ctx, B: ModuleBrand + 'ctx>
        = ()
    where
        'ctx: 'pm;
}

impl TypedPassEffect for MutatesIr {
    type ModuleToken<'pm, 'ctx, B: ModuleBrand + 'ctx>
        = &'pm Module<'ctx, B, Unverified>
    where
        'ctx: 'pm;
}

/// Type-level join on the two-point effect lattice: the composed pipeline is
/// read-only iff every member is. The join is *total* — every effect joins
/// with every other — which is what lets a pipeline's derived effect be folded
/// over abstract member effects (`P::Effect`) without the compiler having to
/// case-split the sealed set. `PreservesVerification` is the identity (it joins
/// to whatever it meets) and `MutatesIr` is absorbing (D8).
pub trait EffectJoin<Rhs: TypedPassEffect> {
    type Out: TypedPassEffect;
}

/// Read-only is the lattice identity: `PreservesVerification ⊔ Rhs = Rhs`.
impl<Rhs: TypedPassEffect> EffectJoin<Rhs> for PreservesVerification {
    type Out = Rhs;
}

/// Transform is absorbing: `MutatesIr ⊔ Rhs = MutatesIr`.
impl<Rhs: TypedPassEffect> EffectJoin<Rhs> for MutatesIr {
    type Out = MutatesIr;
}

/// Token weakening from a pipeline effect to one member's effect. The missing
/// `PreservesVerification: ProvidesToken<MutatesIr>` impl is intentional: a
/// read-only pipeline cannot hand a mutation token to a transform member —
/// unrepresentable rather than checked (D1).
pub trait ProvidesToken<Member: TypedPassEffect>: TypedPassEffect {
    fn member_token<'pm, 'ctx, B: ModuleBrand + 'ctx>(
        token: Self::ModuleToken<'pm, 'ctx, B>,
    ) -> Member::ModuleToken<'pm, 'ctx, B>
    where
        'ctx: 'pm;
}

impl ProvidesToken<PreservesVerification> for PreservesVerification {
    fn member_token<'pm, 'ctx, B: ModuleBrand + 'ctx>(_token: Self::ModuleToken<'pm, 'ctx, B>)
    where
        'ctx: 'pm,
    {
    }
}

impl ProvidesToken<PreservesVerification> for MutatesIr {
    fn member_token<'pm, 'ctx, B: ModuleBrand + 'ctx>(_token: Self::ModuleToken<'pm, 'ctx, B>)
    where
        'ctx: 'pm,
    {
    }
}

impl ProvidesToken<MutatesIr> for MutatesIr {
    fn member_token<'pm, 'ctx, B: ModuleBrand + 'ctx>(
        token: Self::ModuleToken<'pm, 'ctx, B>,
    ) -> Self::ModuleToken<'pm, 'ctx, B>
    where
        'ctx: 'pm,
    {
        token
    }
}

/// Type-level left-fold of a cons list of effect markers through
/// [`JoinsAll`], threading a running accumulator `Acc`. `()` yields the
/// accumulator; `(Head, Tail)` joins `Head` onto `Acc` via
/// [`JoinsAll::JoinOut`] and recurses. Because `JoinsAll` makes the join total
/// over an arbitrary `Rhs`, each step's join is well-formed for abstract member
/// effects without the compiler having to case-split the sealed effect set — a
/// flat `where`-clause fold cannot express this.
pub trait EffectFold<Acc: TypedPassEffect> {
    /// The joined effect of the cons list, starting from `Acc`.
    type Out: TypedPassEffect;
}

impl<Acc: TypedPassEffect> EffectFold<Acc> for () {
    type Out = Acc;
}

impl<Acc, Head, Tail> EffectFold<Acc> for (Head, Tail)
where
    Acc: TypedPassEffect,
    Head: JoinsAll,
    Tail: EffectFold<Head::JoinOut<Acc>>,
{
    type Out = <Tail as EffectFold<Head::JoinOut<Acc>>>::Out;
}

/// Left-fold of a non-empty member-effect list through [`JoinsAll`] to spell a
/// pipeline's derived effect, projected through [`EffectFold`] so the fold is
/// well-formed one pairwise join at a time. Every extra member joins onto the
/// running effect, so the whole pipeline is read-only iff every member is. Used
/// by the [`FunctionPassList`] tuple impls (Task 4 deferred this macro to its
/// first consumer here — Task 5).
macro_rules! join_effects {
    // Cons builder over effect types: (E0, (E1, (..., (En, ())))).
    (@cons $only:ty) => { ($only, ()) };
    (@cons $head:ty, $($tail:ty),+) => {
        ($head, join_effects!(@cons $($tail),+))
    };
    ($($eff:ty),+) => {
        <join_effects!(@cons $($eff),+) as EffectFold<PreservesVerification>>::Out
    };
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

    pub fn is_empty(&self) -> bool {
        self.passes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.passes.len()
    }

    pub fn pipeline_text(&self) -> String {
        let mut text = String::new();
        for pass in &self.passes {
            if !text.is_empty() {
                text.push(',');
            }
            text.push_str(pass.name());
        }
        text
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
        self.passes.push(Box::new(FunctionPassModel {
            name: std::any::type_name::<P>().to_owned(),
            pass,
        }));
    }

    pub fn add_named_pass<P>(&mut self, name: PassName<FunctionPassScope>, pass: P)
    where
        P: ReadOnlyFunctionPass<'ctx, B> + 'ctx,
    {
        self.passes.push(Box::new(FunctionPassModel {
            name: name.as_str().to_owned(),
            pass,
        }));
    }

    pub fn add_pipeline_pass<P>(&mut self, pass: P)
    where
        P: PassPipelineInfo<Scope = FunctionPassScope> + ReadOnlyFunctionPass<'ctx, B> + 'ctx,
    {
        self.add_named_pass(P::PIPELINE_NAME, pass);
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
        self.passes.push(Box::new(FunctionPassModel {
            name: std::any::type_name::<P>().to_owned(),
            pass,
        }));
    }

    pub fn add_named_pass<P>(&mut self, name: PassName<FunctionPassScope>, pass: P)
    where
        P: FunctionPass<'ctx, B> + 'ctx,
    {
        self.passes.push(Box::new(FunctionPassModel {
            name: name.as_str().to_owned(),
            pass,
        }));
    }

    pub fn add_pipeline_pass<P>(&mut self, pass: P)
    where
        P: PassPipelineInfo<Scope = FunctionPassScope> + FunctionPass<'ctx, B> + 'ctx,
    {
        self.add_named_pass(P::PIPELINE_NAME, pass);
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

    pub fn is_empty(&self) -> bool {
        self.passes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.passes.len()
    }

    pub fn pipeline_text(&self) -> String {
        let mut text = String::new();
        for pass in &self.passes {
            if !text.is_empty() {
                text.push(',');
            }
            text.push_str(pass.name());
        }
        text
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
        self.passes.push(Box::new(ModulePassModel {
            name: std::any::type_name::<P>().to_owned(),
            pass,
        }));
    }

    pub fn add_named_pass<P>(&mut self, name: PassName<ModulePassScope>, pass: P)
    where
        P: ReadOnlyModulePass<'ctx, B> + 'ctx,
    {
        self.passes.push(Box::new(ModulePassModel {
            name: name.as_str().to_owned(),
            pass,
        }));
    }

    pub fn add_pipeline_pass<P>(&mut self, pass: P)
    where
        P: PassPipelineInfo<Scope = ModulePassScope> + ReadOnlyModulePass<'ctx, B> + 'ctx,
    {
        self.add_named_pass(P::PIPELINE_NAME, pass);
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
        self.passes.push(Box::new(ModulePassModel {
            name: std::any::type_name::<P>().to_owned(),
            pass,
        }));
    }

    pub fn add_named_pass<P>(&mut self, name: PassName<ModulePassScope>, pass: P)
    where
        P: ModulePass<'ctx, B> + 'ctx,
    {
        self.passes.push(Box::new(ModulePassModel {
            name: name.as_str().to_owned(),
            pass,
        }));
    }

    pub fn add_pipeline_pass<P>(&mut self, pass: P)
    where
        P: PassPipelineInfo<Scope = ModulePassScope> + ModulePass<'ctx, B> + 'ctx,
    {
        self.add_named_pass(P::PIPELINE_NAME, pass);
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

/// A typed pass over one function body. The typed counterpart of
/// [`FunctionPass`]/[`ReadOnlyFunctionPass`]: effect, analysis requirements,
/// and the preservation lower bound are part of the type (D1, D8).
pub trait TypedFunctionPass<'ctx, B: ModuleBrand + 'ctx = Brand<'ctx>> {
    /// Read-only or transform; pipelines derive their effect by joining these.
    type Effect: TypedPassEffect;
    /// Analyses prefetched before `run`; the context accessor is infallible.
    type Requires: FunctionAnalysisList<'ctx, B>;
    /// Static preservation lower bound unioned into the runtime result.
    type MinPreserves: PreservationBound;
    /// Instrumentation-facing name.
    const NAME: &'static str;

    fn run(
        &mut self,
        cx: &mut TypedFunctionPassContext<'_, '_, 'ctx, B, Self::Requires, Self::Effect>,
    ) -> IrResult<PreservedAnalyses>;

    fn is_required(&self) -> bool {
        false
    }
}

/// Instrumentation gate + prefetch + collect + run + MinPreserves union +
/// invalidation for a single typed pass, mirroring one iteration of the erased
/// managers' loop (`PassManager::run`, PassManagerImpl.h). The prefetched
/// results borrow `fam` only for the pass's `run` scope, so `fam` is free again
/// for [`FunctionAnalysisManager::invalidate`] once `run` returns — the module
/// `token` is `Copy` and coerces down to that same scope.
fn run_one_typed_function_pass<'pm, 'ctx, B, P>(
    pass: &mut P,
    token: <P::Effect as TypedPassEffect>::ModuleToken<'pm, 'ctx, B>,
    function: FunctionView<'ctx, B>,
    fam: &mut FunctionAnalysisManager<'ctx, B>,
    instrumentation: Option<&PassInstrumentationCallbacks>,
) -> IrResult<PreservedAnalyses>
where
    B: ModuleBrand + 'ctx,
    P: TypedFunctionPass<'ctx, B>,
{
    let should_run = instrumentation.is_none_or(|callbacks| {
        callbacks.run_before_pass(P::NAME, pass.is_required()) || pass.is_required()
    });
    if !should_run {
        return Ok(PreservedAnalyses::all());
    }
    P::Requires::prefetch(fam, function)?;
    let mut pass_pa = {
        // The results borrow `*fam` only for this block (`'r`), while the
        // module `token` keeps its own longer lifetime `'pm`; the two-lifetime
        // context keeps them apart, so `*fam` is free for `invalidate` below.
        let results = P::Requires::collect(&*fam, function)?;
        let mut cx = TypedFunctionPassContext::new(token, function, results);
        pass.run(&mut cx)?
    };
    P::MinPreserves::apply(&mut pass_pa);
    if let Some(callbacks) = instrumentation {
        callbacks.run_after_pass(P::NAME, &pass_pa);
    }
    fam.invalidate(function.as_function(), &pass_pa)?;
    Ok(pass_pa)
}

/// Dispatch tag distinguishing a leaf [`TypedFunctionPass`] member from a
/// nested [`FunctionPipeline`] member. It is an inferred type parameter on
/// [`FunctionPipelineMember`] rather than an overlap: a leaf pass implements
/// the member trait only at [`LeafMember`], a pipeline only at
/// [`NestedMember`], so the blanket-vs-concrete pair never collides even though
/// nothing stops a downstream type from being both a pass and a pipeline.
#[doc(hidden)]
pub struct LeafMember(());
/// Dispatch tag for a nested [`FunctionPipeline`] member. See [`LeafMember`].
#[doc(hidden)]
pub struct NestedMember(());

/// One member of a typed function pipeline: either a [`TypedFunctionPass`]
/// (via the [`LeafMember`] impl) or a nested [`FunctionPipeline`] (via the
/// [`NestedMember`] impl). `Kind` is always inferred from the member type; do
/// not implement directly — the two provided impls are the whole intended
/// universe.
pub trait FunctionPipelineMember<'ctx, B: ModuleBrand + 'ctx, Kind> {
    type MemberEffect: TypedPassEffect;

    #[doc(hidden)]
    fn run_member<'pm>(
        &mut self,
        token: <Self::MemberEffect as TypedPassEffect>::ModuleToken<'pm, 'ctx, B>,
        function: FunctionView<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
        instrumentation: Option<&PassInstrumentationCallbacks>,
    ) -> IrResult<PreservedAnalyses>;
}

impl<'ctx, B, T> FunctionPipelineMember<'ctx, B, LeafMember> for T
where
    B: ModuleBrand + 'ctx,
    T: TypedFunctionPass<'ctx, B>,
{
    type MemberEffect = T::Effect;

    fn run_member<'pm>(
        &mut self,
        token: <T::Effect as TypedPassEffect>::ModuleToken<'pm, 'ctx, B>,
        function: FunctionView<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
        instrumentation: Option<&PassInstrumentationCallbacks>,
    ) -> IrResult<PreservedAnalyses> {
        run_one_typed_function_pass(self, token, function, fam, instrumentation)
    }
}

mod pass_list_sealed {
    pub trait Sealed {}
}

/// Tuple of pipeline members with a derived joint effect. Mirrors the
/// `FunctionParamList` tuple-schema shape; sealed — arities 1..=8, nest a
/// [`FunctionPipeline`] as a member for longer pipelines.
///
/// The joined effect is the [`join_effects!`] left-fold of the members'
/// effects (through [`EffectFold`]/[`JoinsAll`]), and the `ProvidesToken` chain
/// lets the pipeline weaken its own token to each member. Folding through
/// [`EffectFold`] rather than a flat `where`-clause is what makes naming the
/// joined type legal without the compiler first having to case-split the sealed
/// effect set.
pub trait FunctionPassList<'ctx, B: ModuleBrand + 'ctx, Kinds>: pass_list_sealed::Sealed {
    /// Join of every member's effect: read-only iff all members are.
    type Effect: TypedPassEffect;

    #[doc(hidden)]
    fn run_all<'pm>(
        &mut self,
        token: <Self::Effect as TypedPassEffect>::ModuleToken<'pm, 'ctx, B>,
        function: FunctionView<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
        instrumentation: Option<&PassInstrumentationCallbacks>,
    ) -> IrResult<PreservedAnalyses>;
}

macro_rules! impl_function_pass_list {
    ($($member:ident . $kind:ident . $slot:tt),+) => {
        impl<$($member),+> pass_list_sealed::Sealed for ($($member,)+) {}

        impl<'ctx, B, $($member, $kind),+> FunctionPassList<'ctx, B, ($($kind,)+)>
            for ($($member,)+)
        where
            B: ModuleBrand + 'ctx,
            $($member: FunctionPipelineMember<'ctx, B, $kind>,)+
            join_effects!($(<$member as FunctionPipelineMember<'ctx, B, $kind>>::MemberEffect),+):
                $(ProvidesToken<
                    <$member as FunctionPipelineMember<'ctx, B, $kind>>::MemberEffect,
                > +)+ TypedPassEffect,
        {
            type Effect =
                join_effects!(
                    $(<$member as FunctionPipelineMember<'ctx, B, $kind>>::MemberEffect),+
                );

            fn run_all<'pm>(
                &mut self,
                token: <Self::Effect as TypedPassEffect>::ModuleToken<'pm, 'ctx, B>,
                function: FunctionView<'ctx, B>,
                fam: &mut FunctionAnalysisManager<'ctx, B>,
                instrumentation: Option<&PassInstrumentationCallbacks>,
            ) -> IrResult<PreservedAnalyses> {
                let mut preserved = PreservedAnalyses::all();
                $(
                    let member_token = <Self::Effect as ProvidesToken<
                        <$member as FunctionPipelineMember<'ctx, B, $kind>>::MemberEffect,
                    >>::member_token(token);
                    // Each member invalidates `fam` from its own result inside
                    // `run_member` (see `run_one_typed_function_pass`), so the
                    // next member's prefetch already sees the fresh cache.
                    let pass_pa = FunctionPipelineMember::<'ctx, B, $kind>::run_member(
                        &mut self.$slot,
                        member_token,
                        function,
                        fam,
                        instrumentation,
                    )?;
                    preserved.intersect(pass_pa);
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

/// Statically-composed function pipeline. Built with [`function_pipeline`];
/// the read-only/transform effect and therefore `run`'s module-state
/// signature are derived from the members (D8).
pub struct FunctionPipeline<P> {
    passes: P,
    instrumentation: Option<PassInstrumentationCallbacks>,
}

/// Compose a typed function pipeline from a tuple of passes (or nested
/// pipelines).
pub fn function_pipeline<P>(passes: P) -> FunctionPipeline<P> {
    FunctionPipeline {
        passes,
        instrumentation: None,
    }
}

impl<P> FunctionPipeline<P> {
    pub fn with_instrumentation(mut self, callbacks: PassInstrumentationCallbacks) -> Self {
        self.instrumentation = Some(callbacks);
        self
    }

    /// Run over one function. Takes a verified module; returns
    /// `Module<Verified>` when the derived effect is read-only and
    /// `Module<Unverified>` when any member is a transform. `Kinds` is the
    /// inferred leaf/nested dispatch tuple for the members.
    pub fn run<'ctx, B, F, Kinds>(
        &mut self,
        module: Module<'ctx, B, Verified>,
        function: F,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<
        <<P as FunctionPassList<'ctx, B, Kinds>>::Effect as FunctionPipelineExecution>::OutModule<
            'ctx,
            B,
        >,
    >
    where
        B: ModuleBrand + 'ctx,
        P: FunctionPassList<'ctx, B, Kinds>,
        <P as FunctionPassList<'ctx, B, Kinds>>::Effect: FunctionPipelineExecution,
        F: Into<FunctionView<'ctx, B>>,
    {
        <<P as FunctionPassList<'ctx, B, Kinds>>::Effect as FunctionPipelineExecution>::execute(
            &mut self.passes,
            module,
            function.into(),
            fam,
            self.instrumentation.as_ref(),
        )
    }
}

/// Nested-pipeline member: a whole pipeline runs as one member of an outer
/// list, so the arity-8 tuple cap never binds. A pipeline threads the analysis
/// manager to its members explicitly (unlike a leaf pass, whose `Requires` are
/// prefetched into the typed context), so nesting goes through
/// [`FunctionPipelineMember`] directly rather than the leaf [`TypedFunctionPass`]
/// blanket — the two never overlap because [`FunctionPipeline`] does not
/// implement [`TypedFunctionPass`] (its `run` needs the manager the typed
/// context deliberately withholds).
impl<'ctx, B, P, Kinds> FunctionPipelineMember<'ctx, B, (NestedMember, Kinds)>
    for FunctionPipeline<P>
where
    B: ModuleBrand + 'ctx,
    P: FunctionPassList<'ctx, B, Kinds>,
{
    type MemberEffect = <P as FunctionPassList<'ctx, B, Kinds>>::Effect;

    fn run_member<'pm>(
        &mut self,
        token: <Self::MemberEffect as TypedPassEffect>::ModuleToken<'pm, 'ctx, B>,
        function: FunctionView<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
        instrumentation: Option<&PassInstrumentationCallbacks>,
    ) -> IrResult<PreservedAnalyses> {
        let instrumentation = self.instrumentation.as_ref().or(instrumentation);
        self.passes.run_all(token, function, fam, instrumentation)
    }
}

/// Per-effect `run` dispatch: the read-only path threads the verified module
/// through untouched; the transform path downgrades to unverified first.
/// Implemented by the two effect markers only.
pub trait FunctionPipelineExecution: TypedPassEffect {
    type OutModule<'ctx, B: ModuleBrand + 'ctx>;

    #[doc(hidden)]
    fn execute<'ctx, B, P, Kinds>(
        passes: &mut P,
        module: Module<'ctx, B, Verified>,
        function: FunctionView<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
        instrumentation: Option<&PassInstrumentationCallbacks>,
    ) -> IrResult<Self::OutModule<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
        P: FunctionPassList<'ctx, B, Kinds, Effect = Self>;
}

impl FunctionPipelineExecution for PreservesVerification {
    type OutModule<'ctx, B: ModuleBrand + 'ctx> = Module<'ctx, B, Verified>;

    fn execute<'ctx, B, P, Kinds>(
        passes: &mut P,
        module: Module<'ctx, B, Verified>,
        function: FunctionView<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
        instrumentation: Option<&PassInstrumentationCallbacks>,
    ) -> IrResult<Self::OutModule<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
        P: FunctionPassList<'ctx, B, Kinds, Effect = Self>,
    {
        passes.run_all((), function, fam, instrumentation)?;
        Ok(module)
    }
}

impl FunctionPipelineExecution for MutatesIr {
    type OutModule<'ctx, B: ModuleBrand + 'ctx> = Module<'ctx, B, Unverified>;

    fn execute<'ctx, B, P, Kinds>(
        passes: &mut P,
        module: Module<'ctx, B, Verified>,
        function: FunctionView<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
        instrumentation: Option<&PassInstrumentationCallbacks>,
    ) -> IrResult<Self::OutModule<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
        P: FunctionPassList<'ctx, B, Kinds, Effect = Self>,
    {
        let module = module.unverify();
        passes.run_all(&module, function, fam, instrumentation)?;
        Ok(module)
    }
}

/// A typed pass over one module. The typed counterpart of
/// [`ModulePass`]/[`ReadOnlyModulePass`]: effect, analysis requirements, and
/// the preservation lower bound are part of the type (D1, D8). Mirrors
/// [`TypedFunctionPass`] at module scope.
pub trait TypedModulePass<'ctx, B: ModuleBrand + 'ctx = Brand<'ctx>> {
    /// Read-only or transform; pipelines derive their effect by joining these.
    type Effect: TypedPassEffect;
    /// Analyses prefetched before `run`; the context accessor is infallible.
    type Requires: ModuleAnalysisList<'ctx, B>;
    /// Static preservation lower bound unioned into the runtime result.
    type MinPreserves: PreservationBound;
    /// Instrumentation-facing name.
    const NAME: &'static str;

    fn run(
        &mut self,
        cx: &mut TypedModulePassContext<'_, '_, '_, 'ctx, B, Self::Requires, Self::Effect>,
    ) -> IrResult<PreservedAnalyses>;

    fn is_required(&self) -> bool {
        false
    }
}

/// Instrumentation gate + prefetch + collect + run + MinPreserves union for a
/// single typed module pass, mirroring one iteration of the erased managers'
/// loop (`ModulePassManager::run` above) minus invalidation. The prefetched
/// results borrow `mam` only for the pass's `run` scope, and `fam` is
/// reborrowed at that same scope, so both managers are free again once `run`
/// returns -- the module `token` is `Copy` and coerces down to that same
/// scope, exactly like [`run_one_typed_function_pass`].
///
/// Unlike [`run_one_typed_function_pass`], this runner does *not* invalidate
/// the analysis managers itself: invalidation is owned solely by the
/// [`ModulePassList::run_all`] loop, so that leaf passes, [`ForEachFunction`]
/// (which runs a function pipeline and bypasses this runner entirely), and
/// nested [`ModulePipeline`] members are all invalidated the same way, from
/// the same place. The function level has no such bypassing member -- every
/// function-pipeline member routes through `run_one_typed_function_pass` --
/// so invalidation there is owned by the runner instead. Callers must still
/// invalidate with the returned (MinPreserves-unioned) `pass_pa`.
fn run_one_typed_module_pass<'pm, 'ctx, B, P>(
    pass: &mut P,
    token: <P::Effect as TypedPassEffect>::ModuleToken<'pm, 'ctx, B>,
    module: ModuleView<'ctx, B>,
    mam: &mut ModuleAnalysisManager<'ctx, B>,
    fam: &mut FunctionAnalysisManager<'ctx, B>,
    instrumentation: Option<&PassInstrumentationCallbacks>,
) -> IrResult<PreservedAnalyses>
where
    B: ModuleBrand + 'ctx,
    P: TypedModulePass<'ctx, B>,
{
    let should_run = instrumentation.is_none_or(|callbacks| {
        callbacks.run_before_pass(P::NAME, pass.is_required()) || pass.is_required()
    });
    if !should_run {
        return Ok(PreservedAnalyses::all());
    }
    P::Requires::prefetch(mam, module)?;
    let mut pass_pa = {
        // The results borrow `*mam` only for this block, and `&mut *fam`
        // reborrows `fam` for that same short scope; the module `token`
        // keeps its own longer `'pm` lifetime. Both managers are free again
        // for `invalidate`/`invalidate_module` below.
        let results = P::Requires::collect(&*mam, module)?;
        let mut cx = TypedModulePassContext::new(module, token, results, &mut *fam);
        pass.run(&mut cx)?
    };
    P::MinPreserves::apply(&mut pass_pa);
    if let Some(callbacks) = instrumentation {
        callbacks.run_after_pass(P::NAME, &pass_pa);
    }
    Ok(pass_pa)
}

/// One member of a typed module pipeline: either a [`TypedModulePass`] (via
/// the [`LeafMember`] impl), a nested [`ModulePipeline`] (via the
/// [`NestedMember`] impl), or a [`ForEachFunction`] adaptor. `Kind` is always
/// inferred from the member type; do not implement directly. Mirrors
/// [`FunctionPipelineMember`] at module scope.
pub trait ModulePipelineMember<'ctx, B: ModuleBrand + 'ctx, Kind> {
    type MemberEffect: TypedPassEffect;

    #[doc(hidden)]
    fn run_member<'pm>(
        &mut self,
        token: <Self::MemberEffect as TypedPassEffect>::ModuleToken<'pm, 'ctx, B>,
        module: ModuleView<'ctx, B>,
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
        instrumentation: Option<&PassInstrumentationCallbacks>,
    ) -> IrResult<PreservedAnalyses>;
}

impl<'ctx, B, T> ModulePipelineMember<'ctx, B, LeafMember> for T
where
    B: ModuleBrand + 'ctx,
    T: TypedModulePass<'ctx, B>,
{
    type MemberEffect = T::Effect;

    fn run_member<'pm>(
        &mut self,
        token: <T::Effect as TypedPassEffect>::ModuleToken<'pm, 'ctx, B>,
        module: ModuleView<'ctx, B>,
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
        instrumentation: Option<&PassInstrumentationCallbacks>,
    ) -> IrResult<PreservedAnalyses> {
        run_one_typed_module_pass(self, token, module, mam, fam, instrumentation)
    }
}

/// Runs a typed function pipeline over every function definition in module
/// order. Typed counterpart of [`ModuleToFunctionPassAdaptor`]
/// (`createModuleToFunctionPassAdaptor`, IR/PassManager.h).
pub struct ForEachFunction<P> {
    pipeline: FunctionPipeline<P>,
}

/// Wrap a typed function pipeline so it can run as one member of a
/// [`ModulePipeline`], visiting every function definition in module order.
pub fn for_each_function<P>(pipeline: FunctionPipeline<P>) -> ForEachFunction<P> {
    ForEachFunction { pipeline }
}

// The `Kind` slot is `(Kinds,)` -- a 1-tuple wrapping the inner
// `FunctionPassList` dispatch tuple -- rather than `Kinds` directly. That
// keeps this impl's `Kind` structurally distinct from the leaf `LeafMember`
// impl and the nested `(NestedMember, Kinds)` impl below, so coherence sees
// three non-overlapping shapes (`LeafMember`, `(Kinds,)`, `(NestedMember,
// Kinds)`) instead of a bare `Kinds` that could unify with either.
impl<'ctx, B, P, Kinds> ModulePipelineMember<'ctx, B, (Kinds,)> for ForEachFunction<P>
where
    B: ModuleBrand + 'ctx,
    P: FunctionPassList<'ctx, B, Kinds>,
{
    type MemberEffect = <P as FunctionPassList<'ctx, B, Kinds>>::Effect;

    fn run_member<'pm>(
        &mut self,
        token: <Self::MemberEffect as TypedPassEffect>::ModuleToken<'pm, 'ctx, B>,
        module: ModuleView<'ctx, B>,
        _mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
        instrumentation: Option<&PassInstrumentationCallbacks>,
    ) -> IrResult<PreservedAnalyses> {
        let mut preserved = PreservedAnalyses::all();
        for function in module.iter_functions() {
            if function.entry_block().is_none() {
                continue;
            }
            let pa = self
                .pipeline
                .passes
                .run_all(token, function, fam, instrumentation)?;
            preserved.intersect(pa);
        }
        preserved.preserve_set::<AllAnalysesOnFunction>();
        preserved.preserve::<FunctionAnalysisManagerModuleProxy>();
        Ok(preserved)
    }
}

mod module_pass_list_sealed {
    pub trait Sealed {}
}

/// Tuple of module-pipeline members with a derived joint effect. Mirrors
/// [`FunctionPassList`] at module scope: sealed, arities 1..=8, nest a
/// [`ModulePipeline`] as a member for longer pipelines. `run_all` invalidates
/// both the module and function analysis managers after each member
/// (mirroring the dyn [`ModulePassManager::run`] above), then force-preserves
/// [`AllAnalysesOnModule`].
pub trait ModulePassList<'ctx, B: ModuleBrand + 'ctx, Kinds>:
    module_pass_list_sealed::Sealed
{
    /// Join of every member's effect: read-only iff all members are.
    type Effect: TypedPassEffect;

    #[doc(hidden)]
    fn run_all<'pm>(
        &mut self,
        token: <Self::Effect as TypedPassEffect>::ModuleToken<'pm, 'ctx, B>,
        module: ModuleView<'ctx, B>,
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
        instrumentation: Option<&PassInstrumentationCallbacks>,
    ) -> IrResult<PreservedAnalyses>;
}

macro_rules! impl_module_pass_list {
    ($($member:ident . $kind:ident . $slot:tt),+) => {
        impl<$($member),+> module_pass_list_sealed::Sealed for ($($member,)+) {}

        impl<'ctx, B, $($member, $kind),+> ModulePassList<'ctx, B, ($($kind,)+)>
            for ($($member,)+)
        where
            B: ModuleBrand + 'ctx,
            $($member: ModulePipelineMember<'ctx, B, $kind>,)+
            join_effects!($(<$member as ModulePipelineMember<'ctx, B, $kind>>::MemberEffect),+):
                $(ProvidesToken<
                    <$member as ModulePipelineMember<'ctx, B, $kind>>::MemberEffect,
                > +)+ TypedPassEffect,
        {
            type Effect =
                join_effects!(
                    $(<$member as ModulePipelineMember<'ctx, B, $kind>>::MemberEffect),+
                );

            fn run_all<'pm>(
                &mut self,
                token: <Self::Effect as TypedPassEffect>::ModuleToken<'pm, 'ctx, B>,
                module: ModuleView<'ctx, B>,
                mam: &mut ModuleAnalysisManager<'ctx, B>,
                fam: &mut FunctionAnalysisManager<'ctx, B>,
                instrumentation: Option<&PassInstrumentationCallbacks>,
            ) -> IrResult<PreservedAnalyses> {
                let mut preserved = PreservedAnalyses::all();
                $(
                    let member_token = <Self::Effect as ProvidesToken<
                        <$member as ModulePipelineMember<'ctx, B, $kind>>::MemberEffect,
                    >>::member_token(token);
                    let pass_pa = ModulePipelineMember::<'ctx, B, $kind>::run_member(
                        &mut self.$slot,
                        member_token,
                        module,
                        mam,
                        fam,
                        instrumentation,
                    )?;
                    mam.invalidate(module, &pass_pa)?;
                    fam.invalidate_module(module, &pass_pa)?;
                    preserved.intersect(pass_pa);
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

/// Statically-composed module pipeline. Built with [`module_pipeline`]; the
/// read-only/transform effect and therefore `run`'s module-state signature
/// are derived from the members (D8). Mirrors [`FunctionPipeline`] at module
/// scope.
pub struct ModulePipeline<P> {
    passes: P,
    instrumentation: Option<PassInstrumentationCallbacks>,
}

/// Compose a typed module pipeline from a tuple of passes (or nested
/// pipelines / [`ForEachFunction`] adaptors).
pub fn module_pipeline<P>(passes: P) -> ModulePipeline<P> {
    ModulePipeline {
        passes,
        instrumentation: None,
    }
}

impl<P> ModulePipeline<P> {
    pub fn with_instrumentation(mut self, callbacks: PassInstrumentationCallbacks) -> Self {
        self.instrumentation = Some(callbacks);
        self
    }

    /// Run over the whole module. Takes a verified module; returns
    /// `Module<Verified>` when the derived effect is read-only and
    /// `Module<Unverified>` when any member is a transform. `Kinds` is the
    /// inferred leaf/nested/for-each dispatch tuple for the members.
    pub fn run<'ctx, B, Kinds>(
        &mut self,
        module: Module<'ctx, B, Verified>,
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<
        <<P as ModulePassList<'ctx, B, Kinds>>::Effect as ModulePipelineExecution>::OutModule<
            'ctx,
            B,
        >,
    >
    where
        B: ModuleBrand + 'ctx,
        P: ModulePassList<'ctx, B, Kinds>,
        <P as ModulePassList<'ctx, B, Kinds>>::Effect: ModulePipelineExecution,
    {
        <<P as ModulePassList<'ctx, B, Kinds>>::Effect as ModulePipelineExecution>::execute(
            &mut self.passes,
            module,
            mam,
            fam,
            self.instrumentation.as_ref(),
        )
    }
}

/// Nested-pipeline member: a whole pipeline runs as one member of an outer
/// list, so the arity-8 tuple cap never binds. Mirrors the
/// [`FunctionPipelineMember`] nesting impl at module scope: a pipeline
/// threads the analysis managers to its members explicitly, so nesting goes
/// through [`ModulePipelineMember`] directly rather than the leaf
/// [`TypedModulePass`] blanket -- the two never overlap because
/// [`ModulePipeline`] does not implement [`TypedModulePass`].
impl<'ctx, B, P, Kinds> ModulePipelineMember<'ctx, B, (NestedMember, Kinds)> for ModulePipeline<P>
where
    B: ModuleBrand + 'ctx,
    P: ModulePassList<'ctx, B, Kinds>,
{
    type MemberEffect = <P as ModulePassList<'ctx, B, Kinds>>::Effect;

    fn run_member<'pm>(
        &mut self,
        token: <Self::MemberEffect as TypedPassEffect>::ModuleToken<'pm, 'ctx, B>,
        module: ModuleView<'ctx, B>,
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
        instrumentation: Option<&PassInstrumentationCallbacks>,
    ) -> IrResult<PreservedAnalyses> {
        let instrumentation = self.instrumentation.as_ref().or(instrumentation);
        self.passes
            .run_all(token, module, mam, fam, instrumentation)
    }
}

/// Per-effect `run` dispatch: the read-only path threads the verified module
/// through untouched; the transform path downgrades to unverified first.
/// Implemented by the two effect markers only. Mirrors
/// [`FunctionPipelineExecution`] at module scope.
pub trait ModulePipelineExecution: TypedPassEffect {
    type OutModule<'ctx, B: ModuleBrand + 'ctx>;

    #[doc(hidden)]
    fn execute<'ctx, B, P, Kinds>(
        passes: &mut P,
        module: Module<'ctx, B, Verified>,
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
        instrumentation: Option<&PassInstrumentationCallbacks>,
    ) -> IrResult<Self::OutModule<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
        P: ModulePassList<'ctx, B, Kinds, Effect = Self>;
}

impl ModulePipelineExecution for PreservesVerification {
    type OutModule<'ctx, B: ModuleBrand + 'ctx> = Module<'ctx, B, Verified>;

    fn execute<'ctx, B, P, Kinds>(
        passes: &mut P,
        module: Module<'ctx, B, Verified>,
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
        instrumentation: Option<&PassInstrumentationCallbacks>,
    ) -> IrResult<Self::OutModule<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
        P: ModulePassList<'ctx, B, Kinds, Effect = Self>,
    {
        let view = module.as_view();
        passes.run_all((), view, mam, fam, instrumentation)?;
        Ok(module)
    }
}

impl ModulePipelineExecution for MutatesIr {
    type OutModule<'ctx, B: ModuleBrand + 'ctx> = Module<'ctx, B, Unverified>;

    fn execute<'ctx, B, P, Kinds>(
        passes: &mut P,
        module: Module<'ctx, B, Verified>,
        mam: &mut ModuleAnalysisManager<'ctx, B>,
        fam: &mut FunctionAnalysisManager<'ctx, B>,
        instrumentation: Option<&PassInstrumentationCallbacks>,
    ) -> IrResult<Self::OutModule<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
        P: ModulePassList<'ctx, B, Kinds, Effect = Self>,
    {
        let module = module.unverify();
        let view = module.as_view();
        passes.run_all(&module, view, mam, fam, instrumentation)?;
        Ok(module)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// llvmkit-specific effect-join lock (no upstream analog: upstream pass
    /// managers have no compile-time read-only/transform distinction at all).
    #[test]
    fn effect_join_is_a_lattice_join() {
        fn assert_same<E, F>()
        where
            E: EffectJoin<F, Out = MutatesIr>,
            F: TypedPassEffect,
        {
        }
        fn assert_ro<E, F>()
        where
            E: EffectJoin<F, Out = PreservesVerification>,
            F: TypedPassEffect,
        {
        }
        assert_ro::<PreservesVerification, PreservesVerification>();
        assert_same::<PreservesVerification, MutatesIr>();
        assert_same::<MutatesIr, PreservesVerification>();
        assert_same::<MutatesIr, MutatesIr>();
    }
}
