//! Ports of `llvm/test/Other/pass-pipeline-parsing.ll`: textual pipelines
//! resolved by `parse_module_pass_pipeline`, the `PassBuilder::parsePassPipeline`
//! that `opt -passes=` calls, over the fixture's own `@f`.
//!
//! The `RUN` lines whose check is an error message are ported whole: the
//! same pipeline text, and upstream's message as the error's `Display`. The
//! `RUN` lines whose check is the `-debug-pass-manager` trace ("Running
//! pass: NoOpModulePass") are ported up to the trace: the pipeline resolves,
//! its members are the ones the trace names, in its order, and it runs over
//! `@f`. The trace itself needs pass instrumentation, which llvmkit does not
//! fire yet (`docs/future-work.md`, "Instrumentation wiring").
//!
//! Not ported: the `RUN` lines over the CGSCC and loop layers
//! (`no-op-cgscc`, `no-op-loop`, `cgscc(…)`, `loop(…)`), which llvmkit does
//! not model, and the `-aa-pipeline` / `-passes-ep-*` options, which belong
//! to `opt`, not to the pipeline parser.

use std::cell::Cell;
use std::rc::Rc;

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    Analyses, DominatorTreeAnalysis, DynBrand, DynFunctionPipeline, DynModulePipeline, FnCx,
    FnReport, FunctionAdaptorInvalidation, FunctionPass, Inspect, IrError, IrResult,
    MAX_COMPOSED_PASS_PIPELINE_NESTING, MAX_PASS_PIPELINE_NESTING, Module, ModuleBrand,
    NoOpFunctionPass, NoOpModulePass, PassPipelineElement, PassPipelineTextName,
    PipelineNestingBound, PipelineOutline, RegisteredFunctionPass, RegisteredModulePass,
    UnmodeledPassManager, Unverified, parse_function_pass_pipeline, parse_module_pass_pipeline,
};

/// The fixture's IR: `define void @f()` with a self-loop.
const FIXTURE: &str = "define void @f() {\nentry:\n br label %loop\nloop:\n br label %loop\n}\n";

fn fixture() -> Module<DynBrand, Unverified> {
    parse(FIXTURE)
}

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("the module parses: {error:?}"),
    }
}

/// Resolves `passes`, checks its whole outline (every nested member, in run
/// order), and runs the pipeline over the fixture.
fn runs_with_outline(passes: &str, outline: &[PipelineOutline<'_>]) -> Result<(), IrError> {
    let module = fixture();
    let mut analyses = Analyses::new();
    let mut pipeline: DynModulePipeline<'_, DynBrand> = parse_module_pass_pipeline(passes)?;
    assert_eq!(pipeline.outline(), outline, "the outline of {passes:?}");
    pipeline.run(module.verify()?, &mut analyses)?.verify()?;
    Ok(())
}

const NO_OP_MODULE: PipelineOutline<'static> = PipelineOutline::Pass("no-op-module");
const NO_OP_FUNCTION: PipelineOutline<'static> = PipelineOutline::Pass("no-op-function");

fn nested<'a>(name: &'a str, members: &[PipelineOutline<'a>]) -> PipelineOutline<'a> {
    PipelineOutline::Pipeline {
        name,
        members: members.to_vec(),
    }
}

/// `opt -passes=<passes>` fails with upstream's message `expected`.
fn refused_with(passes: &str, expected: &str) {
    match parse_module_pass_pipeline::<DynBrand>(passes) {
        Ok(pipeline) => panic!("{passes:?} resolved to {pipeline:?}, expected {expected:?}"),
        Err(error) => assert_eq!(error.to_string(), expected, "the error for {passes:?}"),
    }
}

/// Port of `pass-pipeline-parsing.ll`'s `CHECK-TWO-NOOP-MP`,
/// `CHECK-NESTED-TWO-NOOP-MP`, `CHECK-TWO-NOOP-FP`, `CHECK-NESTED-TWO-NOOP-FP`
/// and `CHECK-MIXED-FP-AND-MP` runs, up to the trace (see the module doc):
/// two module passes, the same nested in `module(…)`, two function passes
/// (wrapped in one function adaptor, as upstream wraps them), the same
/// written as `function(…)`, and a function pipeline between module passes.
#[test]
fn no_op_pipelines_resolve_in_the_traced_order() -> Result<(), IrError> {
    runs_with_outline("no-op-module,no-op-module", &[NO_OP_MODULE, NO_OP_MODULE])?;
    runs_with_outline(
        "module(no-op-module,no-op-module)",
        &[nested("module", &[NO_OP_MODULE, NO_OP_MODULE])],
    )?;
    let two_function_passes = [nested("function", &[NO_OP_FUNCTION, NO_OP_FUNCTION])];
    runs_with_outline("no-op-function,no-op-function", &two_function_passes)?;
    runs_with_outline(
        "function(no-op-function,no-op-function)",
        &two_function_passes,
    )?;
    runs_with_outline(
        "no-op-module,function(no-op-function,no-op-function),no-op-module",
        &[
            NO_OP_MODULE,
            nested("function", &[NO_OP_FUNCTION, NO_OP_FUNCTION]),
            NO_OP_MODULE,
        ],
    )?;
    Ok(())
}

/// Port of `pass-pipeline-parsing.ll`'s `CHECK-RERUN-BAD` run: a module
/// pipeline cannot hold a `no-rerun` function adaptor.
#[test]
fn a_no_rerun_function_adaptor_is_refused() {
    refused_with(
        "function<no-rerun>(no-op-function)",
        "cannot have a no-rerun module to function adaptor",
    );
}

/// Port of `pass-pipeline-parsing.ll`'s `CHECK-UNBALANCED1` through
/// `CHECK-UNBALANCED10` and `CHECK-MISSING-COMMA1` runs: unbalanced
/// parentheses, and a closed inner pipeline followed by anything but a
/// comma, are an invalid pipeline.
#[test]
fn unbalanced_pipelines_are_invalid() {
    for passes in [
        "no-op-module)",
        "module(no-op-module))",
        "module(no-op-module",
        "no-op-function)",
        "function(no-op-function))",
        "function(function(no-op-function)))",
        "function(no-op-function",
        "function(function(no-op-function)",
        "no-op-module,)",
        "no-op-function,)",
        "function(no-op-function)function(no-op-function)",
    ] {
        refused_with(passes, &format!("invalid pipeline '{passes}'"));
    }
}

/// Port of `pass-pipeline-parsing.ll`'s `CHECK-EMPTY-INNER-PIPELINE` run:
/// `function()` holds one element with an empty name, an unknown function
/// pass.
#[test]
fn an_empty_inner_pipeline_names_an_empty_pass() {
    refused_with("function()", "unknown function pass ''");
}

/// Port of `pass-pipeline-parsing.ll`'s `CHECK-PIPELINE-ON-MODULE-PASS`,
/// `CHECK-PIPELINE-ON-FUNCTION-PASS` and `CHECK-EMPTY-PIPELINE-ON-PASS`
/// runs: a pass that is not a pass manager cannot carry a pipeline, even an
/// empty one.
#[test]
fn a_pass_cannot_carry_a_pipeline() {
    refused_with(
        "no-op-module(no-op-module,whatever)",
        "invalid use of 'no-op-module' pass as module pipeline",
    );
    refused_with(
        "no-op-function(no-op-function,whatever)",
        "invalid use of 'no-op-function' pass as function pipeline",
    );
    refused_with(
        "no-op-function()",
        "invalid use of 'no-op-function' pass as function pipeline",
    );
}

/// Port of `pass-pipeline-parsing.ll`'s `CHECK-UNKNOWN-MODULE` and
/// `CHECK-UNKNOWN-FUNCTION` runs, less the one through `cgscc(…)`: an
/// unregistered name is an unknown pass of the layer it is resolved at.
#[test]
fn an_unknown_pass_is_named_with_its_layer() {
    refused_with("no-op-module,bad", "unknown module pass 'bad'");
    for passes in [
        "no-op-function,bad",
        "function(bad,pipeline,text)",
        "module(no-op-module,function(bad,pipeline,text))",
        "no-op-module,function(bad,pipeline,text)",
    ] {
        refused_with(passes, "unknown function pass 'bad'");
    }
}

/// A textual pipeline runs the passes it names: `dce` over a module erases
/// the dead `add` in each definition and skips the declaration, as
/// `ModuleToFunctionPassAdaptor::run` skips `F.isDeclaration()`.
///
/// **No upstream counterpart**: llvmkit-specific, the runnable half of the
/// registry. Upstream's `opt -passes=dce` is the same request.
#[test]
fn a_textual_pipeline_runs_its_passes() -> Result<(), IrError> {
    let module = parse(
        "declare void @h()\n\
         define i32 @g(i32 %x) {\nentry:\n  %dead = add i32 %x, 1\n  ret i32 %x\n}\n",
    );
    let mut analyses = Analyses::new();
    let mut pipeline: DynModulePipeline<'_, DynBrand> =
        parse_module_pass_pipeline("instsimplify,dce")?;
    let after = pipeline.run(module.verify()?, &mut analyses)?.verify()?;
    let printed = format!("{after}");
    assert!(!printed.contains("%dead"), "dce erased %dead:\n{printed}");
    assert!(printed.contains("declare void @h()"), "{printed}");
    Ok(())
}

/// `<wrapper>(` nested `depth` deep around `inner`.
fn nested_text(wrapper: &str, depth: usize, inner: &str) -> String {
    format!(
        "{}{inner}{}",
        format!("{wrapper}(").repeat(depth),
        ")".repeat(depth)
    )
}

fn assert_too_deep<T: core::fmt::Debug>(
    result: Result<T, IrError>,
    bound: PipelineNestingBound,
    what: &str,
) {
    match result {
        Err(IrError::PassPipelineTooDeep { bound: passed }) => {
            assert_eq!(passed, bound, "{what}: the bound passed")
        }
        other => panic!("{what}: expected PassPipelineTooDeep, got {other:?}"),
    }
}

/// A function pipeline around `no-op-function`, nested `depth` levels deep.
fn function_pipeline_nested(
    depth: usize,
) -> Result<DynFunctionPipeline<'static, DynBrand>, IrError> {
    let mut pipeline = DynFunctionPipeline::new();
    pipeline.push(NoOpFunctionPass);
    for _ in 0..depth {
        let mut outer = DynFunctionPipeline::new();
        outer.push_pipeline(pipeline)?;
        pipeline = outer;
    }
    Ok(pipeline)
}

/// Resolves `passes` and runs it over the fixture.
fn resolves_and_runs(passes: &str) -> Result<(), IrError> {
    let module = fixture();
    let mut analyses = Analyses::new();
    let mut pipeline: DynModulePipeline<'_, DynBrand> = parse_module_pass_pipeline(passes)?;
    pipeline.run(module.verify()?, &mut analyses)?.verify()?;
    Ok(())
}

/// Text and parsed elements hold at most `MAX_PASS_PIPELINE_NESTING`
/// parentheses open, and composed pipelines at most
/// `MAX_COMPOSED_PASS_PIPELINE_NESTING` levels, one more, the level
/// resolution adds without a parenthesis. Text at its bound resolves and
/// runs in every shape: `function(` nesting, and the two shapes that gain
/// the adaptor level (a bare pass innermost in `module(…)`, and a pipeline
/// that starts with a function pass). One more parenthesis is refused, and
/// so is text nested 200,000 deep, at once, rather than overflowing the
/// stack in resolution, run or drop. The parsed AST's public constructor and
/// each composition entry (`push_pipeline` on both containers,
/// `push_function_pipeline`) hold their bounds.
///
/// **No upstream counterpart**: upstream has no bound, and its recursive
/// parse, run and teardown exhaust the stack on such text
/// (`docs/divergences.md` D44).
#[test]
fn nesting_is_bounded_in_text_and_in_composition() -> Result<(), IrError> {
    let text_limit = MAX_PASS_PIPELINE_NESTING;
    let limit = MAX_COMPOSED_PASS_PIPELINE_NESTING;
    assert_eq!(text_limit + 1, limit);

    resolves_and_runs(&nested_text("function", text_limit, "no-op-function"))?;
    resolves_and_runs(&nested_text("module", text_limit, "no-op-function"))?;
    resolves_and_runs(&format!(
        "no-op-function,{}",
        nested_text("function", text_limit, "no-op-function")
    ))?;
    for depth in [text_limit + 1, 200_000] {
        for wrapper in ["function", "module"] {
            assert_too_deep(
                parse_module_pass_pipeline::<DynBrand>(&nested_text(
                    wrapper,
                    depth,
                    "no-op-function",
                )),
                PipelineNestingBound::Text,
                &format!("{wrapper}( x{depth}"),
            );
        }
    }

    // The AST's public constructor holds the text bound.
    let name = |text: &str| PassPipelineTextName::try_new(text);
    let mut element = PassPipelineElement::new(name("no-op-function")?, [])?;
    for _ in 0..text_limit {
        element = PassPipelineElement::new(name("function")?, [element])?;
    }
    assert_too_deep(
        PassPipelineElement::new(name("function")?, [element]),
        PipelineNestingBound::Text,
        "an element one past the text bound",
    );

    // Composition: a pipeline `limit` levels deep may not be nested again,
    // through any of the three entries.
    assert_too_deep(
        DynFunctionPipeline::<'_, DynBrand>::new().push_pipeline(function_pipeline_nested(limit)?),
        PipelineNestingBound::Composed,
        "DynFunctionPipeline::push_pipeline",
    );
    assert_too_deep(
        DynModulePipeline::<'_, DynBrand>::new().push_function_pipeline(
            function_pipeline_nested(limit)?,
            FunctionAdaptorInvalidation::FromReport,
        ),
        PipelineNestingBound::Composed,
        "DynModulePipeline::push_function_pipeline",
    );

    let mut module_pipeline = DynModulePipeline::<'_, DynBrand>::new();
    module_pipeline.push(NoOpModulePass);
    for _ in 0..limit {
        let mut outer = DynModulePipeline::new();
        outer.push_pipeline(module_pipeline)?;
        module_pipeline = outer;
    }
    assert_too_deep(
        DynModulePipeline::<'_, DynBrand>::new().push_pipeline(module_pipeline),
        PipelineNestingBound::Composed,
        "DynModulePipeline::push_pipeline",
    );
    Ok(())
}

/// The registry lists every registered pass and manager, in
/// `PassRegistry.def` order, and each name resolves back to its pass.
///
/// **No upstream counterpart**: llvmkit-specific. Each registry enum, its
/// `all()` list and its spellings come from one `name_table!` list, so a
/// variant cannot be missing from `all()`; this pins the lists themselves.
#[test]
fn the_registry_resolves_every_name_it_lists() {
    let module_names: Vec<&str> = RegisteredModulePass::all()
        .map(|pass| pass.name().as_str())
        .collect();
    assert_eq!(module_names, ["no-op-module"]);
    let function_names: Vec<&str> = RegisteredFunctionPass::all()
        .map(|pass| pass.name().as_str())
        .collect();
    assert_eq!(function_names, ["dce", "instsimplify", "no-op-function"]);
    for pass in RegisteredModulePass::all() {
        assert_eq!(
            RegisteredModulePass::from_name(pass.name().as_str()),
            Some(pass)
        );
    }
    for pass in RegisteredFunctionPass::all() {
        assert_eq!(
            RegisteredFunctionPass::from_name(pass.name().as_str()),
            Some(pass)
        );
    }
    let managers: Vec<&str> = UnmodeledPassManager::all()
        .map(UnmodeledPassManager::as_str)
        .collect();
    assert_eq!(
        managers,
        [
            "cgscc",
            "coro-cond",
            "loop",
            "loop-mssa",
            "machine-function"
        ]
    );
    for manager in UnmodeledPassManager::all() {
        assert_eq!(
            UnmodeledPassManager::from_name(manager.as_str()),
            Some(manager)
        );
    }
}

/// Counts the functions it runs over.
struct CountFunctions {
    count: Rc<Cell<usize>>,
}

impl<B: ModuleBrand> FunctionPass<B> for CountFunctions {
    type Access = Inspect;
    type Requires = ();
    const NAME: &'static str = "count-functions";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, Inspect, ()>) -> IrResult<FnReport<B>>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        self.count.set(self.count.get() + 1);
        Ok(cx.done())
    }
}

/// A function adaptor runs its pipeline over each definition, in a module
/// that also declares a function, and not over the declaration.
///
/// **No upstream counterpart**: llvmkit-specific. It pins
/// `ModuleToFunctionPassAdaptor::run`'s `if (F.isDeclaration()) continue;`,
/// which no registered pass observes: each is harmless on a declaration.
#[test]
fn a_function_adaptor_skips_declarations() -> Result<(), IrError> {
    let module = parse(
        "declare void @h()\n\
         define void @f() {\nentry:\n  ret void\n}\n\
         define void @g() {\nentry:\n  ret void\n}\n",
    );
    let count = Rc::new(Cell::new(0));
    let mut functions = DynFunctionPipeline::new();
    functions.push(CountFunctions {
        count: Rc::clone(&count),
    });
    let mut pipeline: DynModulePipeline<'_, DynBrand> = DynModulePipeline::new();
    pipeline.push_function_pipeline(functions, FunctionAdaptorInvalidation::FromReport)?;
    let mut analyses = Analyses::new();
    pipeline.run(module.verify()?, &mut analyses)?;
    assert_eq!(count.get(), 2, "@f and @g, not the declaration @h");
    Ok(())
}

/// A function pipeline resolves through `parse_function_pass_pipeline` and
/// runs over one function: `instsimplify` folds `1 + 2` into its user and
/// erases the `add`. A nested `function(…)` runs its members in place, and
/// `dce` then has nothing left to erase.
///
/// **No upstream counterpart**: llvmkit-specific. It is the function-pipeline
/// half of `PassBuilder::parsePassPipeline`, which no upstream test drives
/// directly.
#[test]
fn a_function_pipeline_runs_over_one_function() -> Result<(), IrError> {
    for passes in ["instsimplify,dce", "function(instsimplify),function(dce)"] {
        let module = parse("define i32 @g() {\nentry:\n  %a = add i32 1, 2\n  ret i32 %a\n}\n");
        let g = module.function_dyn("g").expect("@g is defined");
        let mut analyses = Analyses::new();
        let mut pipeline: DynFunctionPipeline<'_, DynBrand> = parse_function_pass_pipeline(passes)?;
        let after = pipeline.run(module.verify()?, g, &mut analyses)?.verify()?;
        let printed = format!("{after}");
        assert!(printed.contains("ret i32 3"), "{passes}:\n{printed}");
        assert!(!printed.contains("%a ="), "{passes}:\n{printed}");
    }
    Ok(())
}

/// A function adaptor invalidates a function's analyses from its pipeline's
/// report, and `function<eager-inv>` drops them all: a dominator tree cached
/// before `function(no-op-function)` survives it, and does not survive
/// `function<eager-inv>(no-op-function)`.
///
/// **No upstream counterpart**: llvmkit-specific. It pins the two arms of
/// `ModuleToFunctionPassAdaptor::run`'s
/// `FAM.invalidate(F, EagerlyInvalidate ? PreservedAnalyses::none() : PassPA)`.
#[test]
fn a_function_adaptor_invalidates_as_its_parameter_says() -> Result<(), IrError> {
    let module = fixture();
    let f = module.function_dyn("f").expect("@f is defined");
    let mut analyses = Analyses::new();
    analyses.register_function_analysis(DominatorTreeAnalysis);
    let verified = module.verify()?;
    analyses
        .function_manager_mut()
        .result::<DominatorTreeAnalysis, _>(verified.view(f))?;

    let mut from_report: DynModulePipeline<'_, DynBrand> =
        parse_module_pass_pipeline("function(no-op-function)")?;
    let after = from_report.run(verified, &mut analyses)?;
    assert!(
        analyses
            .function_manager()
            .cached_result::<DominatorTreeAnalysis, _>(after.view(f))
            .is_some(),
        "a pipeline that preserves everything keeps the dominator tree"
    );

    let mut eager: DynModulePipeline<'_, DynBrand> =
        parse_module_pass_pipeline("function<eager-inv>(no-op-function)")?;
    assert_eq!(
        eager.pass_names().collect::<Vec<_>>(),
        ["function<eager-inv>"]
    );
    let after = eager.run(after.verify()?, &mut analyses)?;
    assert!(
        analyses
            .function_manager()
            .cached_result::<DominatorTreeAnalysis, _>(after.view(f))
            .is_none(),
        "eager invalidation drops the dominator tree"
    );
    Ok(())
}

/// The pass managers of the layers llvmkit does not model are refused by
/// name, and a first name of no layer is an unknown pass or pipeline name;
/// a function pipeline refuses a first name that is not a function pass.
///
/// **No upstream counterpart** for the unmodeled managers: upstream accepts
/// them, and llvmkit refuses them rather than guessing
/// (`docs/divergences.md`). The other messages are upstream's, from
/// `PassBuilder::parsePassPipeline`.
#[test]
fn unmodeled_managers_and_unknown_names_are_refused() {
    refused_with("cgscc(no-op-cgscc)", "pass manager 'cgscc' is not modeled");
    refused_with(
        "coro-cond(no-op-module)",
        "pass manager 'coro-cond' is not modeled",
    );
    refused_with(
        "function(loop(no-op-loop))",
        "pass manager 'loop' is not modeled",
    );
    refused_with(
        "function(loop-mssa(no-op-loop))",
        "pass manager 'loop-mssa' is not modeled",
    );
    refused_with(
        "function(machine-function(no-op-machine-function))",
        "pass manager 'machine-function' is not modeled",
    );
    // `coro-cond` parses its module pipeline first, as upstream does.
    refused_with("coro-cond(bad)", "unknown module pass 'bad'");
    refused_with("bad", "unknown pass name 'bad'");
    refused_with("bad(no-op-module)", "unknown pipeline name 'bad'");
    refused_with("", "unknown pass name ''");
    match parse_function_pass_pipeline::<DynBrand>("no-op-module") {
        Ok(pipeline) => panic!("resolved to {pipeline:?}"),
        Err(error) => assert_eq!(
            error.to_string(),
            "unknown function pass 'no-op-module' in pipeline 'no-op-module'"
        ),
    }
}
