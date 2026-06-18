//! Minimal new-pass-manager analysis substrate coverage.
//!
//! Every test cites its upstream source per Doctrine D11.

use std::cell::Cell;
use std::rc::Rc;

use llvmkit_ir::{
    AllAnalysesOnFunction, AllAnalysesOnModule, CFGAnalyses, DominatorTreeAnalysis,
    FunctionAnalysis, FunctionAnalysisManager, FunctionAnalysisResult, IRBuilder, IrError, Linkage,
    Module, ModuleAnalysis, ModuleAnalysisManager, ModuleAnalysisResult, PreservedAnalyses,
};

#[derive(Clone)]
struct CountFunctionAnalysis {
    runs: Rc<Cell<u32>>,
}

#[derive(Debug)]
struct CountFunctionResult {
    instructions: usize,
}

impl<'ctx> FunctionAnalysis<'ctx> for CountFunctionAnalysis {
    type Result = CountFunctionResult;

    fn run(
        &self,
        function: llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn>,
        _am: &mut FunctionAnalysisManager<'ctx>,
    ) -> llvmkit_ir::IrResult<Self::Result> {
        self.runs.set(self.runs.get() + 1);
        let instructions = function
            .basic_blocks()
            .map(|bb| bb.instructions().len())
            .sum();
        Ok(CountFunctionResult { instructions })
    }
}

impl<'ctx> FunctionAnalysisResult<'ctx> for CountFunctionResult {
    fn invalidate(
        &mut self,
        _function: llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn>,
        pa: &PreservedAnalyses,
    ) -> bool {
        let checker = pa.checker::<CountFunctionAnalysis>();
        !(checker.preserved() || checker.preserved_set::<AllAnalysesOnFunction>())
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

impl<'ctx> ModuleAnalysis<'ctx> for CountModuleAnalysis {
    type Result = CountModuleResult;

    fn run(
        &self,
        module: &'ctx Module<'ctx>,
        _am: &mut ModuleAnalysisManager<'ctx>,
    ) -> llvmkit_ir::IrResult<Self::Result> {
        self.runs.set(self.runs.get() + 1);
        Ok(CountModuleResult {
            functions: module.iter_functions().len(),
        })
    }
}

impl<'ctx> ModuleAnalysisResult<'ctx> for CountModuleResult {
    fn invalidate(&mut self, _module: &'ctx Module<'ctx>, pa: &PreservedAnalyses) -> bool {
        let checker = pa.checker::<CountModuleAnalysis>();
        !(checker.preserved() || checker.preserved_set::<AllAnalysesOnModule>())
    }
}

fn sample_module() -> Result<Module<'static>, IrError> {
    let m = Module::new("analysis");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("f", fn_ty, Linkage::External)?;
    let g = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let h = m.add_function::<()>("h", fn_ty, Linkage::External)?;

    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    b.build_call(g, Vec::<llvmkit_ir::Value>::new(), "")?;
    b.build_call(h, Vec::<llvmkit_ir::Value>::new(), "")?;
    b.build_ret_void();

    for function in [g, h] {
        let entry = function.append_basic_block("entry");
        IRBuilder::new_for::<()>(&m)
            .position_at_end(entry)
            .build_ret_void();
    }
    Ok(m)
}

/// `llvmkit-specific subset`: ports the API-supported assertions from
/// `unittests/IR/PassManagerTest.cpp` `PreservedAnalysesTest` Basic,
/// Preserve, PreserveSets, Intersect, and Abandon. llvmkit has no
/// raw `AnalysisKey` checker, so the final upstream explicit-ID assertions are
/// intentionally omitted.
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
    assert!(all.checker::<CountFunctionAnalysis>().preserved());
    assert!(
        all.checker::<CountFunctionAnalysis>()
            .preserved_set::<AllAnalysesOnFunction>()
    );

    let mut function_set = PreservedAnalyses::none();
    function_set.preserve_set::<AllAnalysesOnFunction>();
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
}

/// Ports `unittests/IR/PassManagerTest.cpp` local function-analysis cache and
/// invalidation behavior: `get_result` runs once, cached lookup does not run,
/// and unpreserved invalidation drops the cached result.
#[test]
fn function_analysis_runs_once_caches_and_invalidates() -> Result<(), IrError> {
    let m = sample_module()?;
    let f = m.function_by_name("f").expect("sample has f");
    let runs = Rc::new(Cell::new(0));
    let mut fam = FunctionAnalysisManager::new();
    fam.register_pass(CountFunctionAnalysis { runs: runs.clone() });

    assert!(fam.get_cached_result::<CountFunctionAnalysis>(f).is_none());
    assert_eq!(fam.get_result::<CountFunctionAnalysis>(f)?.instructions, 3);
    assert_eq!(fam.get_result::<CountFunctionAnalysis>(f)?.instructions, 3);
    assert_eq!(runs.get(), 1);

    fam.invalidate(f, &PreservedAnalyses::all());
    assert!(fam.get_cached_result::<CountFunctionAnalysis>(f).is_some());
    fam.invalidate(f, &PreservedAnalyses::none());
    assert!(fam.get_cached_result::<CountFunctionAnalysis>(f).is_none());
    assert_eq!(fam.get_result::<CountFunctionAnalysis>(f)?.instructions, 3);
    assert_eq!(runs.get(), 2);
    Ok(())
}

/// Ports `unittests/IR/PassManagerTest.cpp` local module-analysis cache and
/// invalidation behavior.
#[test]
fn module_analysis_runs_once_caches_and_invalidates() -> Result<(), IrError> {
    let m = sample_module()?;
    let runs = Rc::new(Cell::new(0));
    let mut mam = ModuleAnalysisManager::new();
    mam.register_pass(CountModuleAnalysis { runs: runs.clone() });

    assert!(mam.get_cached_result::<CountModuleAnalysis>(&m).is_none());
    assert_eq!(mam.get_result::<CountModuleAnalysis>(&m)?.functions, 3);
    assert_eq!(mam.get_result::<CountModuleAnalysis>(&m)?.functions, 3);
    assert_eq!(runs.get(), 1);

    mam.invalidate(&m, &PreservedAnalyses::all());
    assert!(mam.get_cached_result::<CountModuleAnalysis>(&m).is_some());
    mam.invalidate(&m, &PreservedAnalyses::none());
    assert!(mam.get_cached_result::<CountModuleAnalysis>(&m).is_none());
    assert_eq!(mam.get_result::<CountModuleAnalysis>(&m)?.functions, 3);
    assert_eq!(runs.get(), 2);
    Ok(())
}

/// Ports `llvm/lib/IR/Dominators.cpp::DominatorTreeAnalysis::run` and
/// `DominatorTree::invalidate`: the cached tree is preserved by `CFGAnalyses`.
#[test]
fn dominator_tree_analysis_caches_and_cfg_preserves() -> Result<(), IrError> {
    let m = sample_module()?;
    let f = m.function_by_name("f").expect("sample has f");
    let mut fam = FunctionAnalysisManager::new();
    fam.register_pass(DominatorTreeAnalysis);

    let dt = fam.get_result::<DominatorTreeAnalysis>(f)?;
    assert!(dt.is_reachable_from_entry(f.entry_block().expect("body")));

    let mut pa = PreservedAnalyses::none();
    pa.preserve_set::<CFGAnalyses>();
    fam.invalidate(f, &pa);
    assert!(fam.get_cached_result::<DominatorTreeAnalysis>(f).is_some());

    fam.invalidate(f, &PreservedAnalyses::none());
    assert!(fam.get_cached_result::<DominatorTreeAnalysis>(f).is_none());
    Ok(())
}
