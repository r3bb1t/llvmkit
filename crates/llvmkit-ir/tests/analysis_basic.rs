//! Minimal new-pass-manager analysis substrate coverage.
//!
//! Every test cites its upstream source per Doctrine D11.

use std::any::type_name;
use std::cell::Cell;
use std::rc::Rc;

use llvmkit_ir::analysis::{
    AnalysisKeyId, AnalysisSetKeyId, FunctionAnalysisInvalidator,
    FunctionAnalysisManagerModuleProxy, ModuleAnalysisInvalidator,
};
use llvmkit_ir::{
    AllAnalysesOnFunction, AllAnalysesOnModule, CFGAnalyses, DominatorTreeAnalysis,
    FunctionAnalysis, FunctionAnalysisManager, FunctionAnalysisResult, FunctionView, IRBuilder,
    IrError, IrResult, Linkage, Module, ModuleAnalysis, ModuleAnalysisManager,
    ModuleAnalysisResult, ModuleBrand, ModuleView, PreservedAnalyses, Value,
};

#[derive(Clone)]
struct CountFunctionAnalysis {
    runs: Rc<Cell<u32>>,
}

#[derive(Debug)]
struct CountFunctionResult {
    instructions: usize,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionAnalysis<'ctx, B> for CountFunctionAnalysis {
    type Result = CountFunctionResult;

    fn run(
        &self,
        function: FunctionView<'ctx, B>,
        _am: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Self::Result> {
        self.runs.set(self.runs.get() + 1);
        let instructions = function
            .basic_blocks()
            .map(|bb| bb.instruction_count())
            .sum();
        Ok(CountFunctionResult { instructions })
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionAnalysisResult<'ctx, B> for CountFunctionResult {
    fn invalidate(
        &mut self,
        _function: FunctionView<'ctx, B>,
        pa: &PreservedAnalyses,
        _inv: &mut FunctionAnalysisInvalidator<'_, 'ctx, B>,
    ) -> IrResult<bool> {
        let checker = pa.checker::<CountFunctionAnalysis>();
        Ok(!(checker.preserved() || checker.preserved_set::<AllAnalysesOnFunction>()))
    }
}

#[derive(Clone)]
struct CountModuleAnalysis {
    runs: Rc<Cell<u32>>,
}

#[derive(Debug)]
struct CountModuleResult {
    functions: usize,
}

impl<'ctx, B: ModuleBrand + 'ctx> ModuleAnalysis<'ctx, B> for CountModuleAnalysis {
    type Result = CountModuleResult;

    fn run(
        &self,
        module: ModuleView<'ctx, B>,
        _am: &mut ModuleAnalysisManager<'ctx, B>,
    ) -> IrResult<Self::Result> {
        self.runs.set(self.runs.get() + 1);
        Ok(CountModuleResult {
            functions: module.iter_functions().len(),
        })
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> ModuleAnalysisResult<'ctx, B> for CountModuleResult {
    fn invalidate(
        &mut self,
        _module: ModuleView<'ctx, B>,
        pa: &PreservedAnalyses,
        _inv: &mut ModuleAnalysisInvalidator<'_, 'ctx, B>,
    ) -> IrResult<bool> {
        let checker = pa.checker::<CountModuleAnalysis>();
        Ok(!(checker.preserved() || checker.preserved_set::<AllAnalysesOnModule>()))
    }
}

#[derive(Clone, Copy)]
struct DependsOnMissingFunctionAnalysis;

#[derive(Debug)]
struct DependsOnMissingFunctionResult;

impl<'ctx, B: ModuleBrand + 'ctx> FunctionAnalysis<'ctx, B> for DependsOnMissingFunctionAnalysis {
    type Result = DependsOnMissingFunctionResult;

    fn run(
        &self,
        _function: FunctionView<'ctx, B>,
        _am: &mut FunctionAnalysisManager<'ctx, B>,
    ) -> IrResult<Self::Result> {
        Ok(DependsOnMissingFunctionResult)
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionAnalysisResult<'ctx, B>
    for DependsOnMissingFunctionResult
{
    fn invalidate(
        &mut self,
        _function: FunctionView<'ctx, B>,
        _pa: &PreservedAnalyses,
        inv: &mut FunctionAnalysisInvalidator<'_, 'ctx, B>,
    ) -> IrResult<bool> {
        inv.invalidate::<CountFunctionAnalysis>()
    }
}

fn with_sample_module<R, F>(run: F) -> Result<R, IrError>
where
    F: for<'ctx> FnOnce(Module<'ctx>) -> Result<R, IrError>,
{
    Module::with_new("analysis", |module| {
        let f = module
            .add_typed_function::<(), (), _>("f", Linkage::External)?
            .as_function();
        let g = module
            .add_typed_function::<(), (), _>("g", Linkage::External)?
            .as_function();
        let h = module
            .add_typed_function::<(), (), _>("h", Linkage::External)?
            .as_function();

        let entry = f.append_basic_block(&module, "entry");
        let b = IRBuilder::new_for::<()>(&module).position_at_end(entry);
        b.build_call_dyn(g, Vec::<Value>::new(), "")?;
        b.build_call_dyn(h, Vec::<Value>::new(), "")?;
        b.build_ret_void();

        for function in [g, h] {
            let entry = function.append_basic_block(&module, "entry");
            IRBuilder::new_for::<()>(&module)
                .position_at_end(entry)
                .build_ret_void();
        }
        run(module)
    })
}

/// `llvmkit-specific subset`: ports the API-supported assertions from
/// `unittests/IR/PassManagerTest.cpp` `PreservedAnalysesTest` Basic,
/// Preserve, PreserveSets, Intersect, Abandon, and explicit-ID coverage.
#[test]
fn preserved_analyses_checker_behavior() {
    let default = PreservedAnalyses::default();
    assert!(!default.checker::<CountFunctionAnalysis>().preserved());
    assert!(
        !default
            .checker::<CountFunctionAnalysis>()
            .preserved_set::<AllAnalysesOnFunction>()
    );
    assert!(!default.checker::<CountModuleAnalysis>().preserved());
    assert!(
        !default
            .checker::<CountModuleAnalysis>()
            .preserved_set::<AllAnalysesOnModule>()
    );

    let none = PreservedAnalyses::none();
    assert!(!none.checker::<CountFunctionAnalysis>().preserved());
    assert!(
        !none
            .checker::<CountFunctionAnalysis>()
            .preserved_set::<AllAnalysesOnFunction>()
    );

    let all = PreservedAnalyses::all();
    assert!(all.are_all_preserved());
    assert!(all.checker::<CountFunctionAnalysis>().preserved());
    assert!(
        none.checker::<CountFunctionAnalysis>()
            .preserved_when_stateless()
    );
    assert!(
        all.checker::<CountFunctionAnalysis>()
            .preserved_set::<AllAnalysesOnFunction>()
    );

    let function_set = PreservedAnalyses::all_in_set::<AllAnalysesOnFunction>();
    assert!(!function_set.checker::<CountFunctionAnalysis>().preserved());
    assert!(
        function_set
            .checker::<CountFunctionAnalysis>()
            .preserved_set::<AllAnalysesOnFunction>()
    );
    assert!(
        !function_set
            .checker::<CountFunctionAnalysis>()
            .preserved_set::<AllAnalysesOnModule>()
    );

    let cfg_set = PreservedAnalyses::all_in_set::<CFGAnalyses>();
    assert!(!cfg_set.checker::<DominatorTreeAnalysis>().preserved());
    assert!(
        cfg_set
            .checker::<DominatorTreeAnalysis>()
            .preserved_set::<CFGAnalyses>()
    );
    assert!(cfg_set.all_analyses_in_set_preserved::<CFGAnalyses>());

    let mut specific = PreservedAnalyses::none();
    specific.preserve::<CountFunctionAnalysis>();
    assert!(specific.checker::<CountFunctionAnalysis>().preserved());
    assert!(!specific.checker::<CountModuleAnalysis>().preserved());
    specific.preserve::<CountModuleAnalysis>();
    assert!(specific.checker::<CountFunctionAnalysis>().preserved());
    assert!(specific.checker::<CountModuleAnalysis>().preserved());
    specific.preserve::<CountFunctionAnalysis>();
    assert!(specific.checker::<CountFunctionAnalysis>().preserved());
    assert!(specific.checker::<CountModuleAnalysis>().preserved());

    let mut sets = PreservedAnalyses::none();
    sets.preserve_set::<AllAnalysesOnFunction>();
    assert!(
        sets.checker::<CountFunctionAnalysis>()
            .preserved_set::<AllAnalysesOnFunction>()
    );
    assert!(
        !sets
            .checker::<CountModuleAnalysis>()
            .preserved_set::<AllAnalysesOnModule>()
    );
    sets.preserve_set::<AllAnalysesOnModule>();
    assert!(
        sets.checker::<CountFunctionAnalysis>()
            .preserved_set::<AllAnalysesOnFunction>()
    );
    assert!(
        sets.checker::<CountModuleAnalysis>()
            .preserved_set::<AllAnalysesOnModule>()
    );
    sets.preserve::<CountFunctionAnalysis>();
    assert!(
        sets.checker::<CountFunctionAnalysis>()
            .preserved_set::<AllAnalysesOnFunction>()
    );
    assert!(
        sets.checker::<CountModuleAnalysis>()
            .preserved_set::<AllAnalysesOnModule>()
    );
    sets.preserve_set::<AllAnalysesOnModule>();
    assert!(
        sets.checker::<CountFunctionAnalysis>()
            .preserved_set::<AllAnalysesOnFunction>()
    );
    assert!(
        sets.checker::<CountModuleAnalysis>()
            .preserved_set::<AllAnalysesOnModule>()
    );

    let mut pa1 = PreservedAnalyses::none();
    pa1.preserve::<CountFunctionAnalysis>();
    pa1.preserve_set::<AllAnalysesOnModule>();
    let mut pa2 = PreservedAnalyses::none();
    pa2.preserve::<CountFunctionAnalysis>();
    pa2.preserve_set::<AllAnalysesOnFunction>();
    pa2.preserve::<CountModuleAnalysis>();
    pa2.preserve_set::<AllAnalysesOnModule>();
    let mut pa3 = PreservedAnalyses::none();
    pa3.preserve::<CountModuleAnalysis>();
    pa3.preserve_set::<AllAnalysesOnFunction>();

    let mut intersection = pa1.clone();
    intersection.intersect(pa1.clone());
    assert!(intersection.checker::<CountFunctionAnalysis>().preserved());
    assert!(
        !intersection
            .checker::<CountFunctionAnalysis>()
            .preserved_set::<AllAnalysesOnFunction>()
    );
    assert!(!intersection.checker::<CountModuleAnalysis>().preserved());
    assert!(
        intersection
            .checker::<CountModuleAnalysis>()
            .preserved_set::<AllAnalysesOnModule>()
    );

    intersection.intersect(PreservedAnalyses::all());
    assert!(intersection.checker::<CountFunctionAnalysis>().preserved());
    assert!(
        !intersection
            .checker::<CountFunctionAnalysis>()
            .preserved_set::<AllAnalysesOnFunction>()
    );
    assert!(!intersection.checker::<CountModuleAnalysis>().preserved());
    assert!(
        intersection
            .checker::<CountModuleAnalysis>()
            .preserved_set::<AllAnalysesOnModule>()
    );

    intersection.intersect(pa2.clone());
    assert!(intersection.checker::<CountFunctionAnalysis>().preserved());
    assert!(
        !intersection
            .checker::<CountFunctionAnalysis>()
            .preserved_set::<AllAnalysesOnFunction>()
    );
    assert!(!intersection.checker::<CountModuleAnalysis>().preserved());
    assert!(
        intersection
            .checker::<CountModuleAnalysis>()
            .preserved_set::<AllAnalysesOnModule>()
    );

    intersection = pa2.clone();
    intersection.intersect(pa1.clone());
    assert!(intersection.checker::<CountFunctionAnalysis>().preserved());
    assert!(
        !intersection
            .checker::<CountFunctionAnalysis>()
            .preserved_set::<AllAnalysesOnFunction>()
    );
    assert!(!intersection.checker::<CountModuleAnalysis>().preserved());
    assert!(
        intersection
            .checker::<CountModuleAnalysis>()
            .preserved_set::<AllAnalysesOnModule>()
    );

    intersection.intersect(PreservedAnalyses::none());
    assert!(!intersection.checker::<CountFunctionAnalysis>().preserved());
    assert!(
        !intersection
            .checker::<CountFunctionAnalysis>()
            .preserved_set::<AllAnalysesOnFunction>()
    );
    assert!(!intersection.checker::<CountModuleAnalysis>().preserved());
    assert!(
        !intersection
            .checker::<CountModuleAnalysis>()
            .preserved_set::<AllAnalysesOnModule>()
    );

    intersection = pa1.clone();
    intersection.intersect(pa3);
    assert!(!intersection.checker::<CountFunctionAnalysis>().preserved());
    assert!(
        !intersection
            .checker::<CountFunctionAnalysis>()
            .preserved_set::<AllAnalysesOnFunction>()
    );
    assert!(!intersection.checker::<CountModuleAnalysis>().preserved());
    assert!(
        !intersection
            .checker::<CountModuleAnalysis>()
            .preserved_set::<AllAnalysesOnModule>()
    );

    intersection = pa1.clone();
    intersection.intersect(pa2.clone());
    assert!(intersection.checker::<CountFunctionAnalysis>().preserved());
    assert!(
        !intersection
            .checker::<CountFunctionAnalysis>()
            .preserved_set::<AllAnalysesOnFunction>()
    );
    assert!(!intersection.checker::<CountModuleAnalysis>().preserved());
    assert!(
        intersection
            .checker::<CountModuleAnalysis>()
            .preserved_set::<AllAnalysesOnModule>()
    );

    intersection.intersect(PreservedAnalyses::all());
    assert!(intersection.checker::<CountFunctionAnalysis>().preserved());
    assert!(
        !intersection
            .checker::<CountFunctionAnalysis>()
            .preserved_set::<AllAnalysesOnFunction>()
    );
    assert!(!intersection.checker::<CountModuleAnalysis>().preserved());
    assert!(
        intersection
            .checker::<CountModuleAnalysis>()
            .preserved_set::<AllAnalysesOnModule>()
    );

    intersection = PreservedAnalyses::all();
    intersection.intersect(pa1);
    assert!(intersection.checker::<CountFunctionAnalysis>().preserved());
    assert!(
        !intersection
            .checker::<CountFunctionAnalysis>()
            .preserved_set::<AllAnalysesOnFunction>()
    );
    assert!(!intersection.checker::<CountModuleAnalysis>().preserved());
    assert!(
        intersection
            .checker::<CountModuleAnalysis>()
            .preserved_set::<AllAnalysesOnModule>()
    );

    let mut abandoned = PreservedAnalyses::none();
    abandoned.preserve::<CountFunctionAnalysis>();
    abandoned.abandon::<CountFunctionAnalysis>();
    assert!(!abandoned.checker::<CountFunctionAnalysis>().preserved());
    assert!(
        !abandoned
            .checker::<CountFunctionAnalysis>()
            .preserved_when_stateless()
    );
    abandoned.abandon::<CountFunctionAnalysis>();
    assert!(!abandoned.checker::<CountFunctionAnalysis>().preserved());
    abandoned.abandon::<CountModuleAnalysis>();
    assert!(!abandoned.checker::<CountModuleAnalysis>().preserved());
    abandoned.preserve_set::<AllAnalysesOnFunction>();
    abandoned.preserve_set::<AllAnalysesOnModule>();
    assert!(
        !abandoned
            .checker::<CountFunctionAnalysis>()
            .preserved_set::<AllAnalysesOnFunction>()
    );
    assert!(
        !abandoned
            .checker::<CountModuleAnalysis>()
            .preserved_set::<AllAnalysesOnModule>()
    );
    assert!(!abandoned.all_analyses_in_set_preserved::<AllAnalysesOnFunction>());
}

/// Mirrors `llvm/include/llvm/IR/Analysis.h::PreservedAnalyses` explicit-key
/// APIs and abandoned-ID precedence.
#[test]
fn preserved_analyses_explicit_keys_intersect_and_abandon() {
    let key1 = AnalysisKeyId::new(1);
    let key2 = AnalysisKeyId::new(2);
    let set1 = AnalysisSetKeyId::new(10);
    let set2 = AnalysisSetKeyId::new(20);

    let mut keyed = PreservedAnalyses::all_in_set_key(set1);
    assert!(!keyed.checker_for_key(key1).preserved());
    assert!(keyed.checker_for_key(key1).preserved_set_key(set1));
    let none = PreservedAnalyses::none();
    assert!(none.checker_for_key(key1).preserved_when_stateless());

    assert!(keyed.all_analyses_in_set_key_preserved(set1));
    assert!(keyed.checker_for_key(key1).preserved_when_stateless());

    keyed.preserve_key(key1);
    assert!(keyed.checker_for_key(key1).preserved());
    keyed.preserve_set_key(set2);
    assert!(keyed.checker_for_key(key1).preserved_set_key(set2));

    keyed.abandon_key(key1);
    assert!(!keyed.checker_for_key(key1).preserved());
    assert!(!keyed.checker_for_key(key1).preserved_set_key(set1));
    assert!(!keyed.checker_for_key(key1).preserved_set_key(set2));
    assert!(!keyed.checker_for_key(key1).preserved_when_stateless());
    assert!(!keyed.all_analyses_in_set_key_preserved(set1));

    keyed.preserve_key(key1);
    assert!(keyed.checker_for_key(key1).preserved());

    let mut all = PreservedAnalyses::all();
    assert!(all.are_all_preserved());
    all.abandon_key(key1);
    assert!(!all.are_all_preserved());
    assert!(!all.checker_for_key(key1).preserved());
    assert!(all.checker_for_key(key2).preserved());
    assert!(!all.all_analyses_in_set_key_preserved(set1));

    let mut left = PreservedAnalyses::none();
    left.preserve_key(key1);
    left.preserve_key(key2);
    left.preserve_set_key(set1);
    left.preserve_set_key(set2);
    let mut right = PreservedAnalyses::none();
    right.abandon_key(key1);
    right.preserve_key(key2);
    right.preserve_set_key(set2);

    left.intersect(right);
    assert!(!left.checker_for_key(key1).preserved_when_stateless());
    assert!(!left.checker_for_key(key1).preserved());
    assert!(left.checker_for_key(key2).preserved());
    assert!(!left.checker_for_key(key2).preserved_set_key(set1));
    assert!(left.checker_for_key(key2).preserved_set_key(set2));
}

/// Ports `unittests/IR/PassManagerTest.cpp` local function-analysis cache and
/// invalidation behavior: `get_result` runs once, cached lookup does not run,
/// and unpreserved invalidation drops the cached result.
#[test]
fn function_analysis_runs_once_caches_and_invalidates() -> Result<(), IrError> {
    with_sample_module(|m| {
        let f = m.function_by_name("f").expect("sample has f");
        let runs = Rc::new(Cell::new(0));
        let mut fam = FunctionAnalysisManager::new();
        fam.register_pass(CountFunctionAnalysis { runs: runs.clone() });

        assert!(
            fam.get_cached_result::<CountFunctionAnalysis, _>(f)
                .is_none()
        );
        assert_eq!(
            fam.get_result::<CountFunctionAnalysis, _>(f)?.instructions,
            3
        );
        assert_eq!(
            fam.get_result::<CountFunctionAnalysis, _>(f)?.instructions,
            3
        );
        assert_eq!(runs.get(), 1);

        fam.invalidate(f, &PreservedAnalyses::all())?;
        assert!(
            fam.get_cached_result::<CountFunctionAnalysis, _>(f)
                .is_some()
        );
        fam.invalidate(f, &PreservedAnalyses::none())?;
        assert!(
            fam.get_cached_result::<CountFunctionAnalysis, _>(f)
                .is_none()
        );
        assert_eq!(
            fam.get_result::<CountFunctionAnalysis, _>(f)?.instructions,
            3
        );
        assert_eq!(runs.get(), 2);
        Ok(())
    })
}

/// Ports `unittests/IR/PassManagerTest.cpp` local module-analysis cache and
/// invalidation behavior.
#[test]
fn module_analysis_runs_once_caches_and_invalidates() -> Result<(), IrError> {
    with_sample_module(|m| {
        let m = m.verify()?;
        let runs = Rc::new(Cell::new(0));
        let mut mam = ModuleAnalysisManager::new();
        mam.register_pass(CountModuleAnalysis { runs: runs.clone() });

        assert!(
            mam.get_cached_result::<CountModuleAnalysis, _>(m.as_view())
                .is_none()
        );
        assert_eq!(mam.get_result::<CountModuleAnalysis>(&m)?.functions, 3);
        assert_eq!(mam.get_result::<CountModuleAnalysis>(&m)?.functions, 3);
        assert_eq!(runs.get(), 1);

        mam.invalidate(m.as_view(), &PreservedAnalyses::all())?;
        assert!(
            mam.get_cached_result::<CountModuleAnalysis, _>(m.as_view())
                .is_some()
        );
        mam.invalidate(m.as_view(), &PreservedAnalyses::none())?;
        assert!(
            mam.get_cached_result::<CountModuleAnalysis, _>(m.as_view())
                .is_none()
        );
        assert_eq!(mam.get_result::<CountModuleAnalysis>(&m)?.functions, 3);
        assert_eq!(runs.get(), 2);
        Ok(())
    })
}

/// Mirrors LLVM invalidator behavior: dependent invalidation reports a missing
/// cached dependency instead of panicking.
#[test]
fn invalidator_reports_missing_cached_dependency() -> Result<(), IrError> {
    with_sample_module(|m| {
        let f = m.function_by_name("f").expect("sample has f");
        let mut fam = FunctionAnalysisManager::new();
        fam.register_pass(DependsOnMissingFunctionAnalysis);
        let _ = fam.get_result::<DependsOnMissingFunctionAnalysis, _>(f)?;

        let error = fam
            .invalidate(f, &PreservedAnalyses::none())
            .expect_err("missing dependency should be reported");
        assert_eq!(
            error,
            IrError::AnalysisNotCached {
                name: type_name::<CountFunctionAnalysis>(),
            }
        );
        Ok(())
    })
}

/// Mirrors `FunctionAnalysisManagerModuleProxy::Result::invalidate`: module
/// invalidation clears function caches unless the proxy and function-analysis
/// set are preserved.
#[test]
fn module_level_invalidation_honors_fam_proxy_and_function_set() -> Result<(), IrError> {
    with_sample_module(|m| {
        let f = m.function_by_name("f").expect("sample has f");
        let runs = Rc::new(Cell::new(0));
        let mut fam = FunctionAnalysisManager::new();
        fam.register_pass(CountFunctionAnalysis { runs: runs.clone() });
        let _ = fam.get_result::<CountFunctionAnalysis, _>(f)?;
        assert!(
            fam.get_cached_result::<CountFunctionAnalysis, _>(f)
                .is_some()
        );

        fam.invalidate_module(f.module(), &PreservedAnalyses::none())?;
        assert!(
            fam.get_cached_result::<CountFunctionAnalysis, _>(f)
                .is_none()
        );

        let _ = fam.get_result::<CountFunctionAnalysis, _>(f)?;
        let mut pa = PreservedAnalyses::none();
        pa.preserve::<FunctionAnalysisManagerModuleProxy>();
        pa.preserve_set::<AllAnalysesOnFunction>();
        fam.invalidate_module(f.module(), &pa)?;
        assert!(
            fam.get_cached_result::<CountFunctionAnalysis, _>(f)
                .is_some()
        );
        assert_eq!(runs.get(), 2);
        Ok(())
    })
}

/// Ports `llvm/lib/IR/Dominators.cpp::DominatorTreeAnalysis::run` and
/// `DominatorTree::invalidate`: the cached tree is preserved by `CFGAnalyses`.
#[test]
fn dominator_tree_analysis_caches_and_cfg_preserves() -> Result<(), IrError> {
    with_sample_module(|m| {
        let f = m.function_by_name("f").expect("sample has f");
        let mut fam = FunctionAnalysisManager::new();
        fam.register_pass(DominatorTreeAnalysis);

        let dt = fam.get_result::<DominatorTreeAnalysis, _>(f)?;
        assert!(dt.is_reachable_from_entry(f.entry_block().expect("body")));

        let mut pa = PreservedAnalyses::none();
        pa.preserve_set::<CFGAnalyses>();
        fam.invalidate(f, &pa)?;
        assert!(
            fam.get_cached_result::<DominatorTreeAnalysis, _>(f)
                .is_some()
        );

        fam.invalidate(f, &PreservedAnalyses::none())?;
        assert!(
            fam.get_cached_result::<DominatorTreeAnalysis, _>(f)
                .is_none()
        );
        Ok(())
    })
}
