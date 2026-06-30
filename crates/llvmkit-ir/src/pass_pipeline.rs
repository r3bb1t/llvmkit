//! Data-only pass-pipeline names, parser AST, and built-in recipe metadata.
//!
//! This mirrors LLVM's textual-pipeline shape without adding a public
//! `PassBuilder` or constructing any optimization pass.

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

    /// Validates a user-provided static pass name for scoped insertion.
    pub fn try_new(raw: &'static str) -> IrResult<Self> {
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
/// Roadmap module recipe: `default<O0>`.
pub const DEFAULT_O0: PipelineName<ModulePipelineScope> = PipelineName::new("default<O0>");
/// Roadmap module recipe: `default<O1>`.
pub const DEFAULT_O1: PipelineName<ModulePipelineScope> = PipelineName::new("default<O1>");

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

/// Owned validated name at the erased textual parser boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PassPipelineTextName {
    text: String,
}

impl PassPipelineTextName {
    /// Validates a pass-pipeline element name.
    pub fn try_new(name: &str) -> IrResult<Self> {
        validate_pipeline_name(name)?;
        Ok(Self {
            text: name.to_owned(),
        })
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
    if name.is_empty()
        || name
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || matches!(byte, b',' | b'(' | b')'))
    {
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

/// One parsed pass-pipeline element with an optional nested pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PassPipelineElement {
    name: PassPipelineTextName,
    inner_pipeline: Vec<PassPipelineElement>,
}

impl PassPipelineElement {
    /// Constructs an element from an already-validated name.
    pub fn new(name: PassPipelineTextName, inner_pipeline: Vec<Self>) -> Self {
        Self {
            name,
            inner_pipeline,
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
/// Mirrors `PassBuilder.cpp::parsePipelineText` delimiter nesting while keeping
/// name resolution and pass construction out of scope.
pub fn parse_pass_pipeline_text(text: &str) -> IrResult<PassPipeline> {
    if text.is_empty() {
        return Err(IrError::InvalidPassPipeline {
            pipeline: text.to_owned(),
        });
    }

    let mut parser = PipelineParser::new(text);
    let elements = parser.parse_pipeline(false)?;
    if parser.is_at_end() {
        let pipeline = PassPipeline { elements };
        if pipeline.is_empty() {
            Err(IrError::InvalidPassPipeline {
                pipeline: text.to_owned(),
            })
        } else {
            Ok(pipeline)
        }
    } else {
        Err(IrError::InvalidPassPipeline {
            pipeline: text.to_owned(),
        })
    }
}

struct PipelineParser<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> PipelineParser<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, pos: 0 }
    }

    fn is_at_end(&self) -> bool {
        self.pos == self.text.len()
    }

    fn parse_pipeline(&mut self, allow_close: bool) -> IrResult<Vec<PassPipelineElement>> {
        let mut elements = Vec::new();
        loop {
            if self.is_at_end() {
                return if allow_close {
                    Err(self.invalid_pipeline())
                } else {
                    Ok(elements)
                };
            }

            let element = self.parse_element(allow_close)?;
            elements.push(element);

            if self.is_at_end() {
                return if allow_close {
                    Err(self.invalid_pipeline())
                } else {
                    Ok(elements)
                };
            }

            match self.peek_char() {
                Some(',') => {
                    self.consume_char();
                    if self.is_at_end() {
                        return PassPipelineTextName::try_new("").map(|_| elements);
                    }
                }
                Some(')') if allow_close => {
                    self.consume_char();
                    return Ok(elements);
                }
                Some(')') => return Err(self.invalid_pipeline()),
                Some(_) | None => return Err(self.invalid_pipeline()),
            }
        }
    }

    fn parse_element(&mut self, allow_close: bool) -> IrResult<PassPipelineElement> {
        let start = self.pos;
        match self.next_delimiter() {
            Some((delimiter_pos, '(')) => {
                let name = PassPipelineTextName::try_new(&self.text[start..delimiter_pos])?;
                self.pos = delimiter_pos + '('.len_utf8();
                let inner_pipeline = self.parse_pipeline(true)?;
                Ok(PassPipelineElement::new(name, inner_pipeline))
            }
            Some((delimiter_pos, ',')) => {
                let name = PassPipelineTextName::try_new(&self.text[start..delimiter_pos])?;
                self.pos = delimiter_pos;
                Ok(PassPipelineElement::new(name, Vec::new()))
            }
            Some((delimiter_pos, ')')) if allow_close => {
                let name = PassPipelineTextName::try_new(&self.text[start..delimiter_pos])?;
                self.pos = delimiter_pos;
                Ok(PassPipelineElement::new(name, Vec::new()))
            }
            Some(_) => Err(self.invalid_pipeline()),
            None if allow_close => Err(self.invalid_pipeline()),
            None => {
                let name = PassPipelineTextName::try_new(&self.text[start..])?;
                self.pos = self.text.len();
                Ok(PassPipelineElement::new(name, Vec::new()))
            }
        }
    }

    fn next_delimiter(&self) -> Option<(usize, char)> {
        self.text[self.pos..]
            .char_indices()
            .find(|(_, ch)| matches!(ch, ',' | '(' | ')'))
            .map(|(offset, ch)| (self.pos + offset, ch))
    }

    fn peek_char(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    fn consume_char(&mut self) {
        match self.peek_char() {
            Some(ch) => {
                self.pos += ch.len_utf8();
            }
            None => unreachable!("parser consume requires a current character"),
        }
    }

    fn invalid_pipeline(&self) -> IrError {
        IrError::InvalidPassPipeline {
            pipeline: self.text.to_owned(),
        }
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

/// Returns the `default<O0>` module-pipeline recipe.
pub const fn default_o0_pipeline() -> PassPipelineRecipe<ModulePipelineScope, HasOptimizationLevel>
{
    default_pipeline::<OptLevelO0>()
}

/// Returns the `default<O1>` module-pipeline recipe.
pub const fn default_o1_pipeline() -> PassPipelineRecipe<ModulePipelineScope, HasOptimizationLevel>
{
    default_pipeline::<OptLevelO1>()
}
