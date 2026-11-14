//! The name-to-pass half of `PassBuilder` (`PassBuilder.cpp`): resolves a
//! textual pipeline ([`parse_pass_pipeline_text`]) into a runnable
//! [`DynModulePipeline`] or [`DynFunctionPipeline`].
//!
//! Each routine here ports its upstream counterpart arm for arm:
//! [`parse_module_pass_pipeline`] is `parsePassPipeline(ModulePassManager &,
//! StringRef)`, [`parse_function_pass_pipeline`] is
//! `parsePassPipeline(FunctionPassManager &, StringRef)`, and the element
//! walks are `parseModulePass` / `parseFunctionPass`. Two things are narrower
//! than upstream, and each refuses rather than guesses:
//!
//! - **The registry is the passes llvmkit ports.** [`RegisteredModulePass`]
//!   and [`RegisteredFunctionPass`] are the rows of upstream's
//!   `PassRegistry.def` that exist here; any other name is refused with
//!   upstream's "unknown pass" message, as upstream refuses a name it does not
//!   register.
//! - **Only the module and function layers are modeled.** The pass managers
//!   of the other layers (`cgscc`, `coro-cond`, `loop`, `loop-mssa`,
//!   `machine-function`) are refused with [`IrError::UnmodeledPassManager`],
//!   which names the manager.
//!
//! Both are recorded in `docs/divergences.md`.

use core::fmt;

use super::dce::DcePass;
use super::error::{IrError, IrResult};
use super::inst_simplify::InstSimplifyPass;
use super::module::ModuleBrand;
use super::pass_access::Inspect;
use super::pass_context::{FnCx, FnReport, ModCx, ModReport};
use super::pass_manager::{
    DynFunctionPipeline, DynModulePipeline, FunctionAdaptorInvalidation, FunctionPass, ModulePass,
};
use super::pass_pipeline::{
    DCE, FunctionPassScope, INSTSIMPLIFY, ModulePassScope, NO_OP_FUNCTION, NO_OP_MODULE, PassName,
    PassPipelineElement, parse_pass_pipeline_text,
};

/// Mirrors `NoOpModulePass` (`PassBuilder.h`): runs, changes nothing, and
/// preserves every analysis. Registered as `no-op-module`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct NoOpModulePass;

impl<B: ModuleBrand> ModulePass<B> for NoOpModulePass {
    type Access = Inspect;
    type Requires = ();
    const NAME: &'static str = NO_OP_MODULE.as_str();

    fn run<'m, 'ctx>(&mut self, cx: ModCx<'m, '_, '_, 'ctx, B, Inspect, ()>) -> IrResult<ModReport>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        Ok(cx.done())
    }
}

/// Mirrors `NoOpFunctionPass` (`PassBuilder.h`): runs, changes nothing, and
/// preserves every analysis. Registered as `no-op-function`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct NoOpFunctionPass;

impl<B: ModuleBrand> FunctionPass<B> for NoOpFunctionPass {
    type Access = Inspect;
    type Requires = ();
    const NAME: &'static str = NO_OP_FUNCTION.as_str();

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, Inspect, ()>) -> IrResult<FnReport<B>>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        Ok(cx.done())
    }
}

/// A module pass a textual pipeline can name: the `MODULE_PASS` rows of
/// upstream's `PassRegistry.def` that llvmkit ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegisteredModulePass {
    /// `no-op-module`: [`NoOpModulePass`].
    NoOp,
}

impl RegisteredModulePass {
    /// Every registered module pass, in `PassRegistry.def` order.
    pub const ALL: [Self; 1] = [Self::NoOp];

    /// The name a textual pipeline spells this pass with.
    pub const fn name(self) -> PassName<ModulePassScope> {
        match self {
            Self::NoOp => NO_OP_MODULE,
        }
    }

    /// The registered pass named `name`, as a `MODULE_PASS` row matches it
    /// (`Name == NAME`).
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|pass| pass.name().as_str() == name)
    }

    /// Appends a new instance of this pass: the row's `CREATE_PASS`.
    pub fn push_into<'ctx, B: ModuleBrand + 'ctx>(self, pipeline: &mut DynModulePipeline<'ctx, B>) {
        match self {
            Self::NoOp => pipeline.push(NoOpModulePass),
        }
    }
}

/// A function pass a textual pipeline can name: the `FUNCTION_PASS` rows of
/// upstream's `PassRegistry.def` that llvmkit ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegisteredFunctionPass {
    /// `dce`: [`DcePass`].
    Dce,
    /// `instsimplify`: [`InstSimplifyPass`].
    InstSimplify,
    /// `no-op-function`: [`NoOpFunctionPass`].
    NoOp,
}

impl RegisteredFunctionPass {
    /// Every registered function pass, in `PassRegistry.def` order.
    pub const ALL: [Self; 3] = [Self::Dce, Self::InstSimplify, Self::NoOp];

    /// The name a textual pipeline spells this pass with.
    pub const fn name(self) -> PassName<FunctionPassScope> {
        match self {
            Self::Dce => DCE,
            Self::InstSimplify => INSTSIMPLIFY,
            Self::NoOp => NO_OP_FUNCTION,
        }
    }

    /// The registered pass named `name`, as a `FUNCTION_PASS` row matches it
    /// (`Name == NAME`).
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|pass| pass.name().as_str() == name)
    }

    /// Appends a new instance of this pass: the row's `CREATE_PASS`.
    pub fn push_into<'ctx, B: ModuleBrand + 'ctx>(
        self,
        pipeline: &mut DynFunctionPipeline<'ctx, B>,
    ) {
        match self {
            Self::Dce => pipeline.push(DcePass),
            Self::InstSimplify => pipeline.push(InstSimplifyPass),
            Self::NoOp => pipeline.push(NoOpFunctionPass),
        }
    }
}

/// The pass-manager layer a textual pipeline element is resolved at; it
/// names the layer in upstream's diagnostics ("unknown function pass").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PassLayer {
    /// A `ModulePassManager`.
    Module,
    /// A `FunctionPassManager`.
    Function,
}

impl fmt::Display for PassLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Module => "module",
            Self::Function => "function",
        })
    }
}

/// Whether an unknown top-level name was written as a pass or as a pipeline
/// (with an inner pipeline): the word upstream's `parsePassPipeline` uses in
/// "unknown pass name" / "unknown pipeline name".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PipelineEntryKind {
    /// A name with no inner pipeline.
    Pass,
    /// A name with an inner pipeline.
    Pipeline,
}

impl fmt::Display for PipelineEntryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Pass => "pass",
            Self::Pipeline => "pipeline",
        })
    }
}

/// A pass manager upstream's textual pipeline accepts and llvmkit does not
/// model: llvmkit has no CGSCC, loop or machine-function layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnmodeledPassManager {
    /// `cgscc(...)`: the call-graph SCC pass manager.
    Cgscc,
    /// `coro-cond(...)`: the coroutine-conditional module wrapper.
    CoroCond,
    /// `loop(...)`: the loop pass manager.
    Loop,
    /// `loop-mssa(...)`: the loop pass manager with MemorySSA.
    LoopMssa,
    /// `machine-function(...)`: the machine-function pass manager.
    MachineFunction,
}

impl UnmodeledPassManager {
    /// The name a textual pipeline spells this manager with.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cgscc => "cgscc",
            Self::CoroCond => "coro-cond",
            Self::Loop => "loop",
            Self::LoopMssa => "loop-mssa",
            Self::MachineFunction => "machine-function",
        }
    }
}

impl fmt::Display for UnmodeledPassManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Builds the module pipeline `text` names.
///
/// Mirrors `PassBuilder::parsePassPipeline(ModulePassManager &, StringRef)`
/// (`PassBuilder.cpp`): when the first name is not a module-layer name but a
/// function-layer one, the whole pipeline runs over every function, as
/// upstream wraps it in `function(...)`; a first name of neither layer is
/// "unknown pass name" (or "unknown pipeline name" when it carries an inner
/// pipeline).
pub fn parse_module_pass_pipeline<'ctx, B: ModuleBrand + 'ctx>(
    text: &str,
) -> IrResult<DynModulePipeline<'ctx, B>> {
    let pipeline = parse_pass_pipeline_text(text)?;
    let first = pipeline.first();
    let first_name = first.name().as_str();
    let mut module_pipeline = DynModulePipeline::new();
    if is_module_pass_name(first_name) {
        parse_module_pass_elements(&mut module_pipeline, pipeline.elements())?;
    } else if is_function_pass_name(first_name) {
        // Upstream rewrites the pipeline as `function(<pipeline>)`, which
        // `parseModulePass` resolves through `parseFunctionPipelineName`
        // (`function` carries no parameters) into a module-to-function
        // adaptor over the parsed function pipeline: built here directly.
        let mut function_pipeline = DynFunctionPipeline::new();
        parse_function_pass_elements(&mut function_pipeline, pipeline.elements())?;
        module_pipeline
            .push_function_pipeline(function_pipeline, FunctionAdaptorInvalidation::FromReport);
    } else {
        // Upstream's CGSCC, loop-nest, loop and machine-function arms wrap
        // names of layers llvmkit does not model, and no name of those
        // layers is registered here, so the name is unknown.
        let kind = if first.is_leaf() {
            PipelineEntryKind::Pass
        } else {
            PipelineEntryKind::Pipeline
        };
        return Err(IrError::UnknownPipelineEntry {
            kind,
            name: first_name.to_owned(),
        });
    }
    Ok(module_pipeline)
}

/// Builds the function pipeline `text` names.
///
/// Mirrors `PassBuilder::parsePassPipeline(FunctionPassManager &, StringRef)`
/// (`PassBuilder.cpp`): the first name must be a function-layer name, or the
/// text is refused as "unknown function pass '…' in pipeline '…'".
pub fn parse_function_pass_pipeline<'ctx, B: ModuleBrand + 'ctx>(
    text: &str,
) -> IrResult<DynFunctionPipeline<'ctx, B>> {
    let pipeline = parse_pass_pipeline_text(text)?;
    let first_name = pipeline.first().name().as_str();
    if !is_function_pass_name(first_name) {
        return Err(IrError::UnknownPassInPipeline {
            layer: PassLayer::Function,
            name: first_name.to_owned(),
            pipeline: text.to_owned(),
        });
    }
    let mut function_pipeline = DynFunctionPipeline::new();
    parse_function_pass_elements(&mut function_pipeline, pipeline.elements())?;
    Ok(function_pipeline)
}

/// Mirrors `parseModulePassPipeline`: each element in order, stopping at the
/// first error.
fn parse_module_pass_elements<'ctx, B: ModuleBrand + 'ctx>(
    pipeline: &mut DynModulePipeline<'ctx, B>,
    elements: &[PassPipelineElement],
) -> IrResult<()> {
    for element in elements {
        parse_module_pass(pipeline, element)?;
    }
    Ok(())
}

/// Mirrors `parseFunctionPassPipeline`: each element in order, stopping at
/// the first error.
fn parse_function_pass_elements<'ctx, B: ModuleBrand + 'ctx>(
    pipeline: &mut DynFunctionPipeline<'ctx, B>,
    elements: &[PassPipelineElement],
) -> IrResult<()> {
    for element in elements {
        parse_function_pass(pipeline, element)?;
    }
    Ok(())
}

/// Mirrors `PassBuilder::parseModulePass` (`PassBuilder.cpp`).
fn parse_module_pass<'ctx, B: ModuleBrand + 'ctx>(
    pipeline: &mut DynModulePipeline<'ctx, B>,
    element: &PassPipelineElement,
) -> IrResult<()> {
    let name = element.name().as_str();
    let inner = element.inner_pipeline();

    // First handle complex passes like the pass managers which carry
    // pipelines.
    if !inner.is_empty() {
        if name == "module" {
            let mut nested = DynModulePipeline::new();
            parse_module_pass_elements(&mut nested, inner)?;
            pipeline.push_pipeline(nested);
            return Ok(());
        }
        if name == "coro-cond" {
            // Upstream parses the inner module pipeline before wrapping it,
            // so an error inside is reported first.
            let mut nested = DynModulePipeline::<'ctx, B>::new();
            parse_module_pass_elements(&mut nested, inner)?;
            return Err(IrError::UnmodeledPassManager {
                manager: UnmodeledPassManager::CoroCond,
            });
        }
        if name == "cgscc" {
            return Err(IrError::UnmodeledPassManager {
                manager: UnmodeledPassManager::Cgscc,
            });
        }
        if let Some(parameters) = parse_function_pipeline_name(name) {
            if parameters.no_rerun {
                return Err(IrError::NoRerunFunctionAdaptor);
            }
            let mut function_pipeline = DynFunctionPipeline::new();
            parse_function_pass_elements(&mut function_pipeline, inner)?;
            let invalidation = if parameters.eager_invalidate {
                FunctionAdaptorInvalidation::Eager
            } else {
                FunctionAdaptorInvalidation::FromReport
            };
            pipeline.push_function_pipeline(function_pipeline, invalidation);
            return Ok(());
        }
        // Normal passes can't have pipelines.
        return Err(IrError::PassUsedAsPipeline {
            layer: PassLayer::Module,
            name: name.to_owned(),
        });
    }

    // Finally expand the registered passes: the module rows, then a function
    // row wrapped in a module-to-function adaptor.
    if let Some(pass) = RegisteredModulePass::from_name(name) {
        pass.push_into(pipeline);
        return Ok(());
    }
    if let Some(pass) = RegisteredFunctionPass::from_name(name) {
        // `createModuleToFunctionPassAdaptor(CREATE_PASS)`. A one-pass
        // function pipeline in the adaptor runs and invalidates exactly as
        // the bare pass would: its member invalidates from its own report,
        // and the pipeline's report adds only the function-analysis set the
        // adaptor preserves anyway.
        let mut function_pipeline = DynFunctionPipeline::new();
        pass.push_into(&mut function_pipeline);
        pipeline.push_function_pipeline(function_pipeline, FunctionAdaptorInvalidation::FromReport);
        return Ok(());
    }
    Err(IrError::UnknownPass {
        layer: PassLayer::Module,
        name: name.to_owned(),
    })
}

/// Mirrors `PassBuilder::parseFunctionPass` (`PassBuilder.cpp`).
fn parse_function_pass<'ctx, B: ModuleBrand + 'ctx>(
    pipeline: &mut DynFunctionPipeline<'ctx, B>,
    element: &PassPipelineElement,
) -> IrResult<()> {
    let name = element.name().as_str();
    let inner = element.inner_pipeline();

    // First handle complex passes like the pass managers which carry
    // pipelines.
    if !inner.is_empty() {
        if name == "function" {
            let mut nested = DynFunctionPipeline::new();
            parse_function_pass_elements(&mut nested, inner)?;
            pipeline.push_pipeline(nested);
            return Ok(());
        }
        let manager = match name {
            "loop" => Some(UnmodeledPassManager::Loop),
            "loop-mssa" => Some(UnmodeledPassManager::LoopMssa),
            "machine-function" => Some(UnmodeledPassManager::MachineFunction),
            _ => None,
        };
        if let Some(manager) = manager {
            return Err(IrError::UnmodeledPassManager { manager });
        }
        // Normal passes can't have pipelines.
        return Err(IrError::PassUsedAsPipeline {
            layer: PassLayer::Function,
            name: name.to_owned(),
        });
    }

    // Now expand the registered passes.
    if let Some(pass) = RegisteredFunctionPass::from_name(name) {
        pass.push_into(pipeline);
        return Ok(());
    }
    Err(IrError::UnknownPass {
        layer: PassLayer::Function,
        name: name.to_owned(),
    })
}

/// `Name.take_until('<')`: the name before any `<…>` parameters.
fn name_without_parameters(name: &str) -> &str {
    name.split_once('<').map_or(name, |(head, _)| head)
}

/// Mirrors `isModulePassName` (`PassBuilder.cpp`): the pass-manager names a
/// module pipeline accepts, then the registered module passes.
fn is_module_pass_name(name: &str) -> bool {
    name == "module"
        || name == "cgscc"
        || name_without_parameters(name) == "function"
        || name == "coro-cond"
        || RegisteredModulePass::from_name(name).is_some()
}

/// Mirrors `isFunctionPassName` (`PassBuilder.cpp`): the pass-manager names a
/// function pipeline accepts, then the registered function passes.
fn is_function_pass_name(name: &str) -> bool {
    name_without_parameters(name) == "function"
        || name == "loop"
        || name == "loop-mssa"
        || name == "machine-function"
        || RegisteredFunctionPass::from_name(name).is_some()
}

/// The parameters of a `function<…>` module-to-function adaptor.
#[derive(Debug, Clone, Copy, Default)]
struct FunctionPipelineParameters {
    eager_invalidate: bool,
    no_rerun: bool,
}

/// Mirrors `parseFunctionPipelineName` (`PassBuilder.cpp`): `function`, or
/// `function<…>` with `;`-separated `eager-inv` / `no-rerun` parameters.
fn parse_function_pipeline_name(name: &str) -> Option<FunctionPipelineParameters> {
    let mut parameters = FunctionPipelineParameters::default();
    let rest = name.strip_prefix("function")?;
    if rest.is_empty() {
        return Some(parameters);
    }
    let mut rest = rest.strip_prefix('<')?.strip_suffix('>')?;
    while !rest.is_empty() {
        let (front, back) = rest.split_once(';').unwrap_or((rest, ""));
        rest = back;
        match front {
            "eager-inv" => parameters.eager_invalidate = true,
            "no-rerun" => parameters.no_rerun = true,
            _ => return None,
        }
    }
    Some(parameters)
}
