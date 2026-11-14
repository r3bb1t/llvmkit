//! Pass-pipeline names, the textual-pipeline parser and its AST, and the
//! built-in recipe metadata.
//!
//! This is the syntax half of LLVM's textual pipelines: nothing here resolves
//! a name or constructs a pass. `pass_builder.rs` resolves a parsed pipeline
//! into a runnable one ([`crate::parse_module_pass_pipeline`],
//! [`crate::parse_function_pass_pipeline`]).

use core::marker::PhantomData;
use core::str::FromStr;

use super::error::{IrError, IrResult};
use super::optimization_level::{
    OptLevelO0, OptLevelO1, OptimizationLevel, OptimizationLevelMarker,
};

mod sealed {
    pub trait Sealed {}
}

/// Sealed marker trait for typed pipeline recipe scopes.
pub trait PipelineScope: sealed::Sealed + Copy + 'static {
    /// Step type accepted by recipes in this scope.
    type Step: Copy + 'static;
}

/// Sealed marker trait for typed pass-name scopes.
pub trait PassScope: sealed::Sealed + Copy + 'static {}

/// Function-pipeline recipe scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionPipelineScope;

/// Module-pipeline recipe scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModulePipelineScope;

/// Function-pass name scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionPassScope;

/// Module-pass name scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModulePassScope;

impl sealed::Sealed for FunctionPipelineScope {}
impl sealed::Sealed for ModulePipelineScope {}
impl sealed::Sealed for FunctionPassScope {}
impl sealed::Sealed for ModulePassScope {}

impl PipelineScope for FunctionPipelineScope {
    type Step = FunctionPipelineStep;
}
impl PipelineScope for ModulePipelineScope {
    type Step = ModulePipelineStep;
}

impl PassScope for FunctionPassScope {}
impl PassScope for ModulePassScope {}

/// Typed static name for a built-in pipeline recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PipelineName<S: PipelineScope> {
    raw: &'static str,
    _scope: PhantomData<fn() -> S>,
}

impl<S: PipelineScope> PipelineName<S> {
    const fn new(raw: &'static str) -> Self {
        Self {
            raw,
            _scope: PhantomData,
        }
    }

    /// Returns the canonical textual spelling.
    pub const fn as_str(self) -> &'static str {
        self.raw
    }
}

/// Typed static name for a pass at a specific pass-manager layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PassName<S: PassScope> {
    raw: &'static str,
    _scope: PhantomData<fn() -> S>,
}

impl<S: PassScope> PassName<S> {
    const fn new(raw: &'static str) -> Self {
        Self {
            raw,
            _scope: PhantomData,
        }
    }

    /// Validates a user-provided static pass name for scoped insertion: not
    /// empty, and free of whitespace and of the pipeline delimiters `,`, `(`
    /// and `)`. Stricter than a textual element ([`PassPipelineTextName`]),
    /// which may be anything without a delimiter, as upstream's are: a pass
    /// is never registered under an empty or space-holding name.
    pub fn try_new(raw: &'static str) -> IrResult<Self> {
        if raw.is_empty() || raw.bytes().any(|byte| byte.is_ascii_whitespace()) {
            return Err(IrError::InvalidPassPipelineName {
                name: raw.to_owned(),
            });
        }
        validate_pipeline_name(raw)?;
        Ok(Self::new(raw))
    }

    /// Returns the canonical textual spelling.
    pub const fn as_str(self) -> &'static str {
        self.raw
    }
}

/// Roadmap recipe: `cleanup-min`.
pub const CLEANUP_MIN: PipelineName<FunctionPipelineScope> = PipelineName::new("cleanup-min");
/// Roadmap recipe: `cleanup-lift`.
pub const CLEANUP_LIFT: PipelineName<FunctionPipelineScope> = PipelineName::new("cleanup-lift");
/// Roadmap recipe: `cleanup-o1-ish`.
pub const CLEANUP_O1_ISH: PipelineName<FunctionPipelineScope> = PipelineName::new("cleanup-o1-ish");
/// llvmkit-specific module recipe. Named `llvmkit-default<O0>` rather than
/// upstream's `default<O0>` alias because this is a small cleanup subset, not
/// a faithful port of `PassBuilderPipelines.cpp::buildO0DefaultPipeline`
/// (which is not empty). The distinct spelling prevents a silent divergence
/// once a pipeline runner materializes these steps.
pub const DEFAULT_O0: PipelineName<ModulePipelineScope> = PipelineName::new("llvmkit-default<O0>");
/// llvmkit-specific module recipe (see [`DEFAULT_O0`]). Named
/// `llvmkit-default<O1>` rather than upstream's `default<O1>` alias.
pub const DEFAULT_O1: PipelineName<ModulePipelineScope> = PipelineName::new("llvmkit-default<O1>");

/// Built-in function pass name: `instsimplify`.
pub const INSTSIMPLIFY: PassName<FunctionPassScope> = PassName::new("instsimplify");
/// Built-in function pass name: `dce`.
pub const DCE: PassName<FunctionPassScope> = PassName::new("dce");
/// Built-in function pass name: `simplifycfg`.
pub const SIMPLIFYCFG: PassName<FunctionPassScope> = PassName::new("simplifycfg");
/// Built-in function pass name: `instcombine`.
pub const INSTCOMBINE: PassName<FunctionPassScope> = PassName::new("instcombine");
/// Built-in function pass name: `sccp`.
pub const SCCP: PassName<FunctionPassScope> = PassName::new("sccp");
/// Built-in function pass name: `bdce`.
pub const BDCE: PassName<FunctionPassScope> = PassName::new("bdce");
/// Built-in function pass name: `early-cse`.
pub const EARLY_CSE: PassName<FunctionPassScope> = PassName::new("early-cse");
/// Built-in function pass name: `gvn-lite`.
pub const GVN_LITE: PassName<FunctionPassScope> = PassName::new("gvn-lite");
/// Built-in function pass name: `no-op-function` (`NoOpFunctionPass`).
pub const NO_OP_FUNCTION: PassName<FunctionPassScope> = PassName::new("no-op-function");
/// Built-in module pass name: `no-op-module` (`NoOpModulePass`).
pub const NO_OP_MODULE: PassName<ModulePassScope> = PassName::new("no-op-module");

/// How many levels of nested pipelines (nested pipelines and function
/// adaptors) a composed `Dyn` pipeline may hold. Deeper is refused with
/// [`IrError::PassPipelineTooDeep`].
///
/// Text and parsed elements ([`PassPipelineElement`]) are held to one level
/// less: at most [`MAX_PASS_PIPELINE_TEXT_NESTING`] open parentheses. A
/// pipeline resolves to at most one more level than its parentheses, because
/// resolution wraps a bare function pass in a module pipeline (or a pipeline
/// that starts with one) in a function adaptor. So any text the parser
/// accepts resolves within this bound.
///
/// Upstream has no bound. Its parse, run and teardown all recurse once per
/// level, so deep enough text exhausts its stack. llvmkit's resolution, run,
/// outline and drop recurse the same way, so the bound is what keeps a few
/// hundred kilobytes of `function(` from aborting the process. Upstream's own
/// default pipelines nest well under it.
pub const MAX_PASS_PIPELINE_NESTING: usize = 64;

/// How many parentheses pipeline text, or a parsed [`PassPipelineElement`],
/// may hold open at once: one less than [`MAX_PASS_PIPELINE_NESTING`],
/// because resolution can add one level (see there).
pub const MAX_PASS_PIPELINE_TEXT_NESTING: usize = MAX_PASS_PIPELINE_NESTING - 1;

/// Owned validated name at the erased textual parser boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PassPipelineTextName {
    text: String,
}

impl PassPipelineTextName {
    /// Validates a pass-pipeline element name: any text without the
    /// delimiters `,`, `(` and `)`, the text `parsePipelineText` can cut out
    /// of a pipeline as one element's `Name`. That includes the empty name
    /// (`a,,b`, `f()`) and names holding spaces, which upstream resolves
    /// later and refuses as unknown passes.
    pub fn try_new<Name: Into<String>>(name: Name) -> IrResult<Self> {
        let text = name.into();
        validate_pipeline_name(&text)?;
        Ok(Self { text })
    }

    /// Returns the validated name as text.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Consumes the name and returns the owned text.
    pub fn into_string(self) -> String {
        self.text
    }
}

impl FromStr for PassPipelineTextName {
    type Err = IrError;

    fn from_str(name: &str) -> IrResult<Self> {
        Self::try_new(name)
    }
}

fn validate_pipeline_name(name: &str) -> IrResult<()> {
    if name.bytes().any(|byte| matches!(byte, b',' | b'(' | b')')) {
        Err(IrError::InvalidPassPipelineName {
            name: name.to_owned(),
        })
    } else {
        Ok(())
    }
}

/// Erased parsed pass pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PassPipeline {
    elements: Vec<PassPipelineElement>,
}

impl PassPipeline {
    /// Returns top-level parsed pipeline elements.
    pub fn elements(&self) -> &[PassPipelineElement] {
        &self.elements
    }

    /// Returns the first top-level element.
    pub fn first(&self) -> &PassPipelineElement {
        match self.elements.first() {
            Some(first) => first,
            None => unreachable!("PassPipeline is non-empty by construction"),
        }
    }

    /// Returns the number of top-level elements.
    pub fn len(&self) -> usize {
        self.elements.len()
    }

    /// Returns whether the pipeline has no top-level elements.
    ///
    /// Parsed pipelines are non-empty by construction; this method exists to
    /// satisfy Rust collection conventions alongside [`Self::len`].
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    /// Consumes the pipeline and returns its top-level elements.
    pub fn into_elements(self) -> Vec<PassPipelineElement> {
        self.elements
    }
}

/// One parsed pass-pipeline element with an optional nested pipeline, nested
/// at most [`MAX_PASS_PIPELINE_TEXT_NESTING`] deep.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PassPipelineElement {
    name: PassPipelineTextName,
    inner_pipeline: Vec<PassPipelineElement>,
    /// Parentheses below this element's name: 0 for a leaf, one more than
    /// its deepest inner element otherwise.
    depth: usize,
}

impl PassPipelineElement {
    /// Constructs an element from an already-validated name and its inner
    /// pipeline, refused with [`IrError::PassPipelineTooDeep`] when that
    /// would nest more than [`MAX_PASS_PIPELINE_TEXT_NESTING`] deep, the
    /// bound text is held to. So no element, parsed or built, is deeper than
    /// the recursion its formatting, comparison and drop can afford.
    pub fn new<Inner>(name: PassPipelineTextName, inner_pipeline: Inner) -> IrResult<Self>
    where
        Inner: IntoIterator<Item = Self>,
    {
        let inner_pipeline: Vec<Self> = inner_pipeline.into_iter().collect();
        let depth = match inner_pipeline.iter().map(|inner| inner.depth).max() {
            None => 0,
            Some(deepest) => deepest + 1,
        };
        if depth > MAX_PASS_PIPELINE_TEXT_NESTING {
            return Err(IrError::PassPipelineTooDeep);
        }
        Ok(Self {
            name,
            inner_pipeline,
            depth,
        })
    }

    /// An element with no inner pipeline.
    fn leaf(name: PassPipelineTextName) -> Self {
        Self {
            name,
            inner_pipeline: Vec::new(),
            depth: 0,
        }
    }

    /// Returns this element's validated textual name.
    pub fn name(&self) -> &PassPipelineTextName {
        &self.name
    }

    /// Returns this element's nested pipeline.
    pub fn inner_pipeline(&self) -> &[Self] {
        &self.inner_pipeline
    }

    /// Returns `true` when the element has no nested pipeline.
    pub fn is_leaf(&self) -> bool {
        self.inner_pipeline.is_empty()
    }
}

/// Parses textual pass-pipeline syntax into an erased AST.
///
/// Mirrors `PassBuilder::parsePipelineText` (`PassBuilder.cpp`): the text up
/// to each `,`, `(` or `)` is one element's name, `(` opens that element's
/// inner pipeline, and a run of `)` closes inner pipelines, after which only
/// `,` or the end may follow. Names are not resolved here, and an empty name
/// is an element like any other (`""` parses as one element named `""`), as
/// upstream's are: [`crate::parse_module_pass_pipeline`] and
/// [`crate::parse_function_pass_pipeline`] refuse it as an unknown pass.
/// Unbalanced parentheses, and a closed inner pipeline followed by anything
/// but `,`, are [`IrError::InvalidPassPipeline`], upstream's `std::nullopt`.
pub fn parse_pass_pipeline_text(text: &str) -> IrResult<PassPipeline> {
    let invalid = || IrError::InvalidPassPipeline {
        pipeline: text.to_owned(),
    };
    // Upstream keeps a stack of pointers to the pipelines being filled, the
    // outermost at the bottom. Here the outermost is `root`, and each open
    // element, with its inner pipeline so far, sits on `open` until its `)`
    // adds it to the pipeline below. Nothing else is added below it while it
    // is open, so each element keeps the place it was named at.
    let mut root = Vec::new();
    let mut open: Vec<(PassPipelineTextName, Vec<PassPipelineElement>)> = Vec::new();
    let mut rest = text;
    loop {
        let position = rest.find([',', '(', ')']);
        let (name, separator) = match position {
            Some(position) => (&rest[..position], rest[position..].chars().next()),
            None => (rest, None),
        };
        // No delimiter can be in the text before the first delimiter.
        let name = PassPipelineTextName {
            text: name.to_owned(),
        };
        let Some(separator) = separator else {
            // A single terminating name: done.
            filling(&mut root, &mut open).push(PassPipelineElement::leaf(name));
            break;
        };
        rest = &rest[name.text.len() + separator.len_utf8()..];
        match separator {
            ',' => {
                // Just a name ending in a comma: continue.
                filling(&mut root, &mut open).push(PassPipelineElement::leaf(name));
            }
            '(' => {
                // Continue in the element's inner pipeline, within the
                // nesting bound (an llvmkit bound; upstream has none).
                if open.len() == MAX_PASS_PIPELINE_TEXT_NESTING {
                    return Err(IrError::PassPipelineTooDeep);
                }
                open.push((name, Vec::new()));
            }
            _ => {
                filling(&mut root, &mut open).push(PassPipelineElement::leaf(name));
                // Close inner pipelines greedily, to avoid empty names
                // between the parentheses. Closing the outermost pipeline is
                // unbalanced.
                loop {
                    let (name, inner) = open.pop().ok_or_else(invalid)?;
                    // Within the bound: `open` never held more than it.
                    let element = PassPipelineElement::new(name, inner)?;
                    filling(&mut root, &mut open).push(element);
                    match rest.strip_prefix(')') {
                        Some(after) => rest = after,
                        None => break,
                    }
                }
                if rest.is_empty() {
                    break;
                }
                // The end of an inner pipeline is always followed by a comma.
                rest = rest.strip_prefix(',').ok_or_else(invalid)?;
            }
        }
    }
    if !open.is_empty() {
        // Unbalanced parentheses.
        return Err(invalid());
    }
    Ok(PassPipeline { elements: root })
}

/// The pipeline being filled: the innermost open element's, or the
/// outermost.
fn filling<'a>(
    root: &'a mut Vec<PassPipelineElement>,
    open: &'a mut [(PassPipelineTextName, Vec<PassPipelineElement>)],
) -> &'a mut Vec<PassPipelineElement> {
    match open.last_mut() {
        Some((_, inner)) => inner,
        None => root,
    }
}

/// A function-pipeline recipe step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FunctionPipelineStep {
    /// A nested named function-pipeline recipe.
    Pipeline(PipelineName<FunctionPipelineScope>),
    /// A function pass name.
    Pass(PassName<FunctionPassScope>),
}

/// A module-pipeline recipe step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModulePipelineStep {
    /// A function-pipeline adaptor boundary.
    FunctionPipeline(PipelineName<FunctionPipelineScope>),
    /// A module pass name.
    ModulePass(PassName<ModulePassScope>),
}

/// Sealed marker for whether a recipe carries an optimization level.
pub trait RecipeLevelState: sealed::Sealed + Copy + 'static {}

/// Marker for recipes that are independent of a high-level optimization level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NoOptimizationLevel;

/// Marker value for recipes tied to a high-level optimization level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HasOptimizationLevel {
    level: OptimizationLevel,
}

impl HasOptimizationLevel {
    /// Creates an optimization-level marker value.
    pub const fn new(level: OptimizationLevel) -> Self {
        Self { level }
    }

    /// Returns the recipe optimization level.
    pub const fn level(self) -> OptimizationLevel {
        self.level
    }
}

impl sealed::Sealed for NoOptimizationLevel {}
impl sealed::Sealed for HasOptimizationLevel {}

impl RecipeLevelState for NoOptimizationLevel {}
impl RecipeLevelState for HasOptimizationLevel {}

/// Typed built-in pass-pipeline recipe metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PassPipelineRecipe<S: PipelineScope, L: RecipeLevelState> {
    name: PipelineName<S>,
    level: L,
    steps: &'static [S::Step],
    _scope: PhantomData<fn() -> S>,
}

impl<S, L> PassPipelineRecipe<S, L>
where
    S: PipelineScope,
    L: RecipeLevelState,
{
    /// Returns the recipe name.
    pub const fn name(self) -> PipelineName<S> {
        self.name
    }

    /// Returns the typed step list.
    pub const fn steps(self) -> &'static [S::Step] {
        self.steps
    }

    /// Returns whether this recipe has no steps.
    pub const fn is_empty(self) -> bool {
        self.steps.is_empty()
    }
}

impl<S> PassPipelineRecipe<S, HasOptimizationLevel>
where
    S: PipelineScope,
{
    /// Returns the recipe optimization level.
    pub const fn level(self) -> OptimizationLevel {
        self.level.level()
    }
}

const CLEANUP_MIN_STEPS: &[FunctionPipelineStep] = &[
    FunctionPipelineStep::Pass(INSTSIMPLIFY),
    FunctionPipelineStep::Pass(DCE),
    FunctionPipelineStep::Pass(SIMPLIFYCFG),
];

const CLEANUP_LIFT_STEPS: &[FunctionPipelineStep] = &[
    FunctionPipelineStep::Pass(INSTCOMBINE),
    FunctionPipelineStep::Pass(SIMPLIFYCFG),
    FunctionPipelineStep::Pass(SCCP),
    FunctionPipelineStep::Pass(INSTCOMBINE),
    FunctionPipelineStep::Pass(DCE),
    FunctionPipelineStep::Pass(BDCE),
    FunctionPipelineStep::Pass(SIMPLIFYCFG),
];

const CLEANUP_O1_ISH_STEPS: &[FunctionPipelineStep] = &[
    FunctionPipelineStep::Pipeline(CLEANUP_LIFT),
    FunctionPipelineStep::Pass(EARLY_CSE),
    FunctionPipelineStep::Pass(GVN_LITE),
    FunctionPipelineStep::Pass(DCE),
];

const DEFAULT_O0_STEPS: &[ModulePipelineStep] = &[];
const DEFAULT_O1_STEPS: &[ModulePipelineStep] =
    &[ModulePipelineStep::FunctionPipeline(CLEANUP_O1_ISH)];

/// Returns the `cleanup-min` function-pipeline recipe.
pub const fn cleanup_min_pipeline() -> PassPipelineRecipe<FunctionPipelineScope, NoOptimizationLevel>
{
    PassPipelineRecipe {
        name: CLEANUP_MIN,
        level: NoOptimizationLevel,
        steps: CLEANUP_MIN_STEPS,
        _scope: PhantomData,
    }
}

/// Returns the `cleanup-lift` function-pipeline recipe.
pub const fn cleanup_lift_pipeline()
-> PassPipelineRecipe<FunctionPipelineScope, NoOptimizationLevel> {
    PassPipelineRecipe {
        name: CLEANUP_LIFT,
        level: NoOptimizationLevel,
        steps: CLEANUP_LIFT_STEPS,
        _scope: PhantomData,
    }
}

/// Returns the `cleanup-o1-ish` function-pipeline recipe.
pub const fn cleanup_o1_ish_pipeline()
-> PassPipelineRecipe<FunctionPipelineScope, NoOptimizationLevel> {
    PassPipelineRecipe {
        name: CLEANUP_O1_ISH,
        level: NoOptimizationLevel,
        steps: CLEANUP_O1_ISH_STEPS,
        _scope: PhantomData,
    }
}

/// Sealed selector for default-pipeline recipes available in this milestone.
pub trait DefaultPipelineLevel: OptimizationLevelMarker + sealed::Sealed {
    /// The default-pipeline alias name.
    const RECIPE_NAME: PipelineName<ModulePipelineScope>;
    /// The default-pipeline typed steps.
    const STEPS: &'static [ModulePipelineStep];
}

impl sealed::Sealed for OptLevelO0 {}
impl sealed::Sealed for OptLevelO1 {}

impl DefaultPipelineLevel for OptLevelO0 {
    const RECIPE_NAME: PipelineName<ModulePipelineScope> = DEFAULT_O0;
    const STEPS: &'static [ModulePipelineStep] = DEFAULT_O0_STEPS;
}
impl DefaultPipelineLevel for OptLevelO1 {
    const RECIPE_NAME: PipelineName<ModulePipelineScope> = DEFAULT_O1;
    const STEPS: &'static [ModulePipelineStep] = DEFAULT_O1_STEPS;
}

/// Returns a supported default-pipeline recipe.
pub const fn default_pipeline<L>() -> PassPipelineRecipe<ModulePipelineScope, HasOptimizationLevel>
where
    L: DefaultPipelineLevel,
{
    PassPipelineRecipe {
        name: L::RECIPE_NAME,
        level: HasOptimizationLevel::new(L::LEVEL),
        steps: L::STEPS,
        _scope: PhantomData,
    }
}

/// Returns the `llvmkit-default<O0>` module-pipeline recipe.
pub const fn default_o0_pipeline() -> PassPipelineRecipe<ModulePipelineScope, HasOptimizationLevel>
{
    default_pipeline::<OptLevelO0>()
}

/// Returns the `llvmkit-default<O1>` module-pipeline recipe.
pub const fn default_o1_pipeline() -> PassPipelineRecipe<ModulePipelineScope, HasOptimizationLevel>
{
    default_pipeline::<OptLevelO1>()
}
