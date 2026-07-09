//! Data-only pass-pipeline substrate coverage.
//!
//! Every test cites its upstream source per Doctrine D11.

use llvmkit_ir::{
    BDCE, CLEANUP_LIFT, CLEANUP_MIN, CLEANUP_O1_ISH, DCE, DEFAULT_O0, DEFAULT_O1, EARLY_CSE,
    FunctionPassScope, FunctionPipelineStep, GVN_LITE, HasOptimizationLevel, INSTCOMBINE,
    INSTSIMPLIFY, IrError, IrResult, ModulePassScope, ModulePipelineStep, NoOptimizationLevel,
    OptLevelO0, OptLevelO1, OptLevelOs, OptimizationLevel, OptimizationLevelMarker, PassName,
    PassPipelineRecipe, PassPipelineTextName, SCCP, SIMPLIFYCFG, cleanup_lift_pipeline,
    cleanup_min_pipeline, cleanup_o1_ish_pipeline, default_o0_pipeline, default_o1_pipeline,
    default_pipeline, parse_pass_pipeline_text,
};

fn assert_invalid_optimization_level(name: &str) {
    match name.parse::<OptimizationLevel>() {
        Err(IrError::InvalidOptimizationLevel { level }) => assert_eq!(level, name),
        other => panic!("expected InvalidOptimizationLevel for {name:?}, got {other:?}"),
    }
}

fn assert_invalid_pipeline_name(name: &str) {
    match PassPipelineTextName::try_new(name) {
        Err(IrError::InvalidPassPipelineName { name: actual }) => assert_eq!(actual, name),
        other => panic!("expected InvalidPassPipelineName for {name:?}, got {other:?}"),
    }
}

/// Port of `llvm/include/llvm/Passes/OptimizationLevel.h` and
/// `llvm/lib/Passes/OptimizationLevel.cpp`: public optimization levels map to
/// the upstream speed/size pairs and predicates.
#[test]
fn optimization_level_matches_upstream_constants() {
    let cases = [
        (OptimizationLevel::O0, 0, 0, false, false),
        (OptimizationLevel::O1, 1, 0, true, false),
        (OptimizationLevel::O2, 2, 0, true, false),
        (OptimizationLevel::O3, 3, 0, true, false),
        (OptimizationLevel::Os, 2, 1, false, true),
        (OptimizationLevel::Oz, 2, 2, false, true),
    ];

    assert_eq!(OptimizationLevel::default(), OptimizationLevel::O2);
    for (level, speed, size, optimizes_speed, optimizes_size) in cases {
        assert_eq!(level.speed_level(), speed);
        assert_eq!(level.size_level(), size);
        assert_eq!(level.is_optimizing_for_speed(), optimizes_speed);
        assert_eq!(level.is_optimizing_for_size(), optimizes_size);
    }
}

/// `llvmkit-specific`, anchored on
/// `llvm/include/llvm/Passes/OptimizationLevel.h`: static marker types project
/// runtime optimization levels into Rust's type system.
#[test]
fn optimization_level_markers_project_static_levels() {
    assert_eq!(OptLevelO1::LEVEL, OptimizationLevel::O1);
    assert_eq!(OptLevelOs::LEVEL, OptimizationLevel::Os);
}

/// Port of `llvm/lib/Passes/PassBuilder.cpp::parseOptLevel`: only upstream's
/// exact optimization-level spellings parse, and display round-trips them.
#[test]
fn optimization_level_parses_upstream_names() {
    let cases = [
        ("O0", OptimizationLevel::O0),
        ("O1", OptimizationLevel::O1),
        ("O2", OptimizationLevel::O2),
        ("O3", OptimizationLevel::O3),
        ("Os", OptimizationLevel::Os),
        ("Oz", OptimizationLevel::Oz),
    ];

    for (text, level) in cases {
        assert_eq!(text.parse::<OptimizationLevel>(), Ok(level));
        assert_eq!(level.to_string(), text);
    }
    assert_invalid_optimization_level("o1");
    assert_invalid_optimization_level("O4");
}

/// `llvmkit-specific`, anchored on `llvm/include/llvm/Passes/PassBuilder.h`
/// lines 323-345: textual names validate at the erased parser boundary before
/// future pass resolution.
#[test]
fn pass_pipeline_text_name_rejects_invalid_text() -> IrResult<()> {
    for name in ["", "bad name", "bad,name", "bad(name)", "bad\nname"] {
        assert_invalid_pipeline_name(name);
    }

    assert_eq!(
        PassPipelineTextName::try_new("llvmkit-default<O1>")?.as_str(),
        "llvmkit-default<O1>"
    );
    assert_eq!(
        PassPipelineTextName::try_new("early-cse")?.as_str(),
        "early-cse"
    );
    assert_eq!(
        PassPipelineTextName::try_new("gvn-lite")?.as_str(),
        "gvn-lite"
    );
    Ok(())
}

/// `llvmkit-specific`: scoped pass names use the same validation as erased
/// pipeline text names while preserving their pass-manager layer in the type.
#[test]
fn scoped_pass_name_rejects_invalid_text() -> IrResult<()> {
    assert_eq!(
        PassName::<FunctionPassScope>::try_new("early-cse")?.as_str(),
        "early-cse"
    );
    match PassName::<ModulePassScope>::try_new("bad,name") {
        Err(IrError::InvalidPassPipelineName { name }) => assert_eq!(name, "bad,name"),
        other => panic!("expected InvalidPassPipelineName, got {other:?}"),
    }
    Ok(())
}

/// Port of `llvm/include/llvm/Passes/PassBuilder.h` lines 323-345 and
/// `llvm/lib/Passes/PassBuilder.cpp::parsePipelineText`: nested textual
/// pipelines are syntax-only data at this milestone.
#[test]
fn pass_pipeline_parser_preserves_nested_shape() -> IrResult<()> {
    let pipeline = parse_pass_pipeline_text("module(function(instcombine,sroa),dce)")?;
    assert_eq!(pipeline.len(), 1);

    let module = pipeline.first();
    assert_eq!(module.name().as_str(), "module");
    assert_eq!(module.inner_pipeline().len(), 2);

    let function = &module.inner_pipeline()[0];
    assert_eq!(function.name().as_str(), "function");
    assert_eq!(function.inner_pipeline().len(), 2);
    assert_eq!(function.inner_pipeline()[0].name().as_str(), "instcombine");
    assert_eq!(function.inner_pipeline()[1].name().as_str(), "sroa");

    let dce = &module.inner_pipeline()[1];
    assert_eq!(dce.name().as_str(), "dce");
    assert!(dce.is_leaf());
    Ok(())
}

/// Port of `llvm/lib/Passes/PassBuilder.cpp::parsePipelineText`: invalid
/// nesting fails, and llvmkit deliberately rejects empty element names early.
#[test]
fn pass_pipeline_parser_rejects_invalid_or_empty_pipelines() {
    match parse_pass_pipeline_text("") {
        Err(IrError::InvalidPassPipeline { pipeline }) => assert_eq!(pipeline, ""),
        other => panic!("expected InvalidPassPipeline for empty pipeline, got {other:?}"),
    }
    for text in ["module(function(instcombine)", "module)function"] {
        match parse_pass_pipeline_text(text) {
            Err(IrError::InvalidPassPipeline { pipeline }) => assert_eq!(pipeline, text),
            other => panic!("expected InvalidPassPipeline for {text:?}, got {other:?}"),
        }
    }
    match parse_pass_pipeline_text("module(function())") {
        Err(IrError::InvalidPassPipelineName { name }) => assert_eq!(name, ""),
        other => panic!("expected InvalidPassPipelineName for empty nested name, got {other:?}"),
    }
}

fn assert_cleanup_recipe(
    recipe: PassPipelineRecipe<llvmkit_ir::FunctionPipelineScope, NoOptimizationLevel>,
    name: llvmkit_ir::PipelineName<llvmkit_ir::FunctionPipelineScope>,
    steps: &[FunctionPipelineStep],
) {
    assert_eq!(recipe.name(), name);
    assert_eq!(recipe.steps(), steps);
    assert_eq!(recipe.is_empty(), steps.is_empty());
}

fn assert_default_recipe(
    recipe: PassPipelineRecipe<llvmkit_ir::ModulePipelineScope, HasOptimizationLevel>,
    name: llvmkit_ir::PipelineName<llvmkit_ir::ModulePipelineScope>,
    level: OptimizationLevel,
    steps: &[ModulePipelineStep],
) {
    assert_eq!(recipe.name(), name);
    assert_eq!(recipe.level(), level);
    assert_eq!(recipe.steps(), steps);
    assert_eq!(recipe.is_empty(), steps.is_empty());
}

/// `llvmkit-specific subset`, anchored on `ROADMAP.md` lines 348-356 and
/// `llvm/lib/Passes/PassRegistry.def` lines 259-264: roadmap aliases are typed
/// allocation-free data and do not instantiate optimization passes.
#[test]
fn roadmap_recipes_are_typed_data_only() {
    assert_cleanup_recipe(
        cleanup_min_pipeline(),
        CLEANUP_MIN,
        &[
            FunctionPipelineStep::Pass(INSTSIMPLIFY),
            FunctionPipelineStep::Pass(DCE),
            FunctionPipelineStep::Pass(SIMPLIFYCFG),
        ],
    );
    assert_cleanup_recipe(
        cleanup_lift_pipeline(),
        CLEANUP_LIFT,
        &[
            FunctionPipelineStep::Pass(INSTCOMBINE),
            FunctionPipelineStep::Pass(SIMPLIFYCFG),
            FunctionPipelineStep::Pass(SCCP),
            FunctionPipelineStep::Pass(INSTCOMBINE),
            FunctionPipelineStep::Pass(DCE),
            FunctionPipelineStep::Pass(BDCE),
            FunctionPipelineStep::Pass(SIMPLIFYCFG),
        ],
    );
    assert_cleanup_recipe(
        cleanup_o1_ish_pipeline(),
        CLEANUP_O1_ISH,
        &[
            FunctionPipelineStep::Pipeline(CLEANUP_LIFT),
            FunctionPipelineStep::Pass(EARLY_CSE),
            FunctionPipelineStep::Pass(GVN_LITE),
            FunctionPipelineStep::Pass(DCE),
        ],
    );

    assert_default_recipe(
        default_o0_pipeline(),
        DEFAULT_O0,
        OptimizationLevel::O0,
        &[],
    );
    assert_default_recipe(
        default_o1_pipeline(),
        DEFAULT_O1,
        OptimizationLevel::O1,
        &[ModulePipelineStep::FunctionPipeline(CLEANUP_O1_ISH)],
    );
    assert_default_recipe(
        default_pipeline::<OptLevelO0>(),
        DEFAULT_O0,
        OptimizationLevel::O0,
        &[],
    );
    assert_default_recipe(
        default_pipeline::<OptLevelO1>(),
        DEFAULT_O1,
        OptimizationLevel::O1,
        &[ModulePipelineStep::FunctionPipeline(CLEANUP_O1_ISH)],
    );
}
