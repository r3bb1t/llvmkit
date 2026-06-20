//! Minimal module/function pass-manager coverage.
//!
//! Every test cites its upstream source per Doctrine D11.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use llvmkit_ir::{
    DominatorTreeAnalysis, FunctionAnalysis, FunctionAnalysisInvalidator, FunctionAnalysisManager,
    FunctionAnalysisResult, FunctionPassManager, IRBuilder, IrError, Linkage, Module,
    ModuleAnalysisManager, ModuleBrand, ModulePassManager, ModuleToFunctionPassAdaptor,
    PreservedAnalyses, PreservesVerification, ReadOnlyFunctionPass, ReadOnlyFunctionPassContext,
    ReadOnlyModulePass, ReadOnlyModulePassContext,
};

fn with_sample_module<R, F>(run: F) -> Result<R, IrError>
where
    F: for<'ctx> FnOnce(Module<'ctx>) -> Result<R, IrError>,
{
    Module::with_new("pm", |m| {
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let g = m.add_function::<(), _>("g", fn_ty, Linkage::External)?;
        let h = m.add_function::<(), _>("h", fn_ty, Linkage::External)?;

        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        b.build_call(g, Vec::<llvmkit_ir::Value>::new(), "")?;
        b.build_call(h, Vec::<llvmkit_ir::Value>::new(), "")?;
        b.build_ret_void();

        for function in [g, h] {
            let entry = function.append_basic_block(&m, "entry");
            IRBuilder::new_for::<()>(&m)
                .position_at_end(entry)
                .build_ret_void();
        }
        let _decl = m.add_function::<(), _>("decl", fn_ty, Linkage::External)?;
        run(m)
    })
}

struct RecordingModulePass {
    name: &'static str,
    order: Rc<RefCell<Vec<&'static str>>>,
    preserved: PreservedAnalyses,
}

impl<'ctx, B: ModuleBrand> ReadOnlyModulePass<'ctx, B> for RecordingModulePass {
    fn run(
        &mut self,
        _cx: &mut ReadOnlyModulePassContext<'_, 'ctx, B>,
    ) -> llvmkit_ir::IrResult<PreservedAnalyses> {
        self.order.borrow_mut().push(self.name);
        Ok(self.preserved.clone())
    }
}

struct RecordingFunctionPass {
    names: Rc<RefCell<Vec<String>>>,
    query_dt: bool,
}

impl<'ctx> ReadOnlyFunctionPass<'ctx> for RecordingFunctionPass {
    fn run(
        &mut self,
        cx: &mut ReadOnlyFunctionPassContext<'_, 'ctx>,
    ) -> llvmkit_ir::IrResult<PreservedAnalyses> {
        let function = cx.function();
        self.names.borrow_mut().push(function.name().to_owned());
        if self.query_dt {
            let dt = cx.analysis::<DominatorTreeAnalysis>()?;
            assert!(dt.is_reachable_from_entry(function.entry_block().expect("definition")));
        }
        Ok(PreservedAnalyses::all())
    }
}

#[derive(Clone)]
struct CountingFunctionAnalysis {
    runs: Rc<Cell<u32>>,
}

#[derive(Debug)]
struct CountingFunctionResult {
    instructions: usize,
}

impl<'ctx> FunctionAnalysis<'ctx> for CountingFunctionAnalysis {
    type Result = CountingFunctionResult;

    fn run(
        &self,
        function: llvmkit_ir::FunctionView<'ctx>,
        _am: &mut FunctionAnalysisManager<'ctx>,
    ) -> llvmkit_ir::IrResult<Self::Result> {
        self.runs.set(self.runs.get() + 1);
        let instructions = function
            .basic_blocks()
            .map(|bb| bb.instruction_count())
            .sum();
        Ok(CountingFunctionResult { instructions })
    }
}

impl<'ctx> FunctionAnalysisResult<'ctx> for CountingFunctionResult {
    fn invalidate(
        &mut self,
        _function: llvmkit_ir::FunctionView<'ctx>,
        pa: &PreservedAnalyses,
        _inv: &mut FunctionAnalysisInvalidator<'_, 'ctx>,
    ) -> llvmkit_ir::IrResult<bool> {
        let checker = pa.checker::<CountingFunctionAnalysis>();
        Ok(!(checker.preserved() || checker.preserved_set::<llvmkit_ir::AllAnalysesOnFunction>()))
    }
}

struct AnalyzingFunctionPass {
    run_count: Rc<Cell<u32>>,
    analyzed_instr_count: Rc<Cell<usize>>,
}

impl<'ctx> ReadOnlyFunctionPass<'ctx> for AnalyzingFunctionPass {
    fn run(
        &mut self,
        cx: &mut ReadOnlyFunctionPassContext<'_, 'ctx>,
    ) -> llvmkit_ir::IrResult<PreservedAnalyses> {
        self.run_count.set(self.run_count.get() + 1);
        let result = cx.analysis::<CountingFunctionAnalysis>()?;
        self.analyzed_instr_count
            .set(self.analyzed_instr_count.get() + result.instructions);
        Ok(PreservedAnalyses::all())
    }
}

struct InvalidateNamedFunctionPass {
    name: &'static str,
}

impl<'ctx> ReadOnlyFunctionPass<'ctx> for InvalidateNamedFunctionPass {
    fn run(
        &mut self,
        cx: &mut ReadOnlyFunctionPassContext<'_, 'ctx>,
    ) -> llvmkit_ir::IrResult<PreservedAnalyses> {
        let function = cx.function();
        if function.name() == self.name {
            Ok(PreservedAnalyses::none())
        } else {
            Ok(PreservedAnalyses::all())
        }
    }
}

/// `llvmkit-specific subset` of
/// `unittests/IR/PassManagerTest.cpp::TEST_F(PassManagerTest, Basic)`:
/// module passes run in insertion order and intersect their
/// `PreservedAnalyses`. llvmkit does not yet model the upstream proxy
/// invalidation/RequireAnalysisPass counters from that test.
#[test]
fn module_pass_manager_runs_in_order() -> Result<(), IrError> {
    with_sample_module(|m| {
        let order = Rc::new(RefCell::new(Vec::new()));
        let mut mpm = ModulePassManager::<_, PreservesVerification>::new_read_only();
        mpm.add_pass(RecordingModulePass {
            name: "a",
            order: order.clone(),
            preserved: PreservedAnalyses::all(),
        });
        mpm.add_pass(RecordingModulePass {
            name: "b",
            order: order.clone(),
            preserved: PreservedAnalyses::none(),
        });
        let mut mam = ModuleAnalysisManager::new();
        let mut fam = FunctionAnalysisManager::new();

        let _ = mpm.run(m.verify()?, &mut mam, &mut fam)?;
        assert_eq!(&*order.borrow(), &["a", "b"]);
        Ok(())
    })
}

/// `llvmkit-specific subset` of `PassManagerTest.cpp::Basic`: ports the
/// supported function-pass run counters, function-analysis cache counts, and
/// one-function invalidation behavior. llvmkit lacks the upstream
/// module/function proxy invalidation and `RequireAnalysisPass` APIs.
#[test]
fn module_pass_manager_counts_supported_cache_and_invalidation() -> Result<(), IrError> {
    with_sample_module(|m| {
        let analysis_runs = Rc::new(Cell::new(0));
        let run_count1 = Rc::new(Cell::new(0));
        let instr_count1 = Rc::new(Cell::new(0usize));
        let run_count2 = Rc::new(Cell::new(0));
        let instr_count2 = Rc::new(Cell::new(0usize));
        let run_count3 = Rc::new(Cell::new(0));
        let instr_count3 = Rc::new(Cell::new(0usize));

        let mut first = FunctionPassManager::<_, PreservesVerification>::new_read_only();
        first.add_pass(AnalyzingFunctionPass {
            run_count: run_count1.clone(),
            analyzed_instr_count: instr_count1.clone(),
        });

        let mut second = FunctionPassManager::<_, PreservesVerification>::new_read_only();
        second.add_pass(AnalyzingFunctionPass {
            run_count: run_count2.clone(),
            analyzed_instr_count: instr_count2.clone(),
        });
        second.add_pass(InvalidateNamedFunctionPass { name: "f" });

        let mut third = FunctionPassManager::<_, PreservesVerification>::new_read_only();
        third.add_pass(AnalyzingFunctionPass {
            run_count: run_count3.clone(),
            analyzed_instr_count: instr_count3.clone(),
        });

        let mut mpm = ModulePassManager::<_, PreservesVerification>::new_read_only();
        mpm.add_pass(ModuleToFunctionPassAdaptor::new(first));
        mpm.add_pass(ModuleToFunctionPassAdaptor::new(second));
        mpm.add_pass(ModuleToFunctionPassAdaptor::new(third));
        let mut mam = ModuleAnalysisManager::new();
        let mut fam = FunctionAnalysisManager::new();
        fam.register_pass(CountingFunctionAnalysis {
            runs: analysis_runs.clone(),
        });

        mpm.run(m.verify()?, &mut mam, &mut fam)?;

        assert_eq!(run_count1.get(), 3);
        assert_eq!(instr_count1.get(), 5);
        assert_eq!(run_count2.get(), 3);
        assert_eq!(instr_count2.get(), 5);
        assert_eq!(run_count3.get(), 3);
        assert_eq!(instr_count3.get(), 5);
        assert_eq!(analysis_runs.get(), 4);
        Ok(())
    })
}

/// `llvmkit-specific subset` of `PassManagerTest.cpp` proxy invalidation:
/// module passes that do not preserve function analyses clear the function
/// analysis cache for the whole module.
#[test]
fn module_pass_invalidates_function_analysis_cache() -> Result<(), IrError> {
    with_sample_module(|m| {
        let f = m.function_by_name("f").expect("sample has f");
        let runs = Rc::new(Cell::new(0));
        let mut fam = FunctionAnalysisManager::new();
        fam.register_pass(CountingFunctionAnalysis { runs: runs.clone() });
        assert!(
            fam.get_cached_result::<CountingFunctionAnalysis, _>(f)
                .is_none()
        );
        assert_eq!(
            fam.get_result::<CountingFunctionAnalysis, _>(f)?
                .instructions,
            3
        );

        let mut mpm = ModulePassManager::<_, PreservesVerification>::new_read_only();
        mpm.add_pass(RecordingModulePass {
            name: "invalidate",
            order: Rc::new(RefCell::new(Vec::new())),
            preserved: PreservedAnalyses::none(),
        });
        let mut mam = ModuleAnalysisManager::new();
        let _ = mpm.run(m.verify()?, &mut mam, &mut fam)?;

        assert!(
            fam.get_cached_result::<CountingFunctionAnalysis, _>(f)
                .is_none()
        );
        assert_eq!(runs.get(), 1);
        Ok(())
    })
}

/// `llvmkit-specific subset` of `PassManagerTest.cpp`: the
/// module-to-function adaptor runs function passes over definitions only and
/// skips declarations. llvmkit lacks loop/CGSCC adaptors and proxy analyses.
#[test]
fn module_to_function_adaptor_runs_defined_functions_only() -> Result<(), IrError> {
    with_sample_module(|m| {
        let names = Rc::new(RefCell::new(Vec::new()));
        let mut fpm = FunctionPassManager::<_, PreservesVerification>::new_read_only();
        fpm.add_pass(RecordingFunctionPass {
            names: names.clone(),
            query_dt: false,
        });
        let mut mpm = ModulePassManager::<_, PreservesVerification>::new_read_only();
        mpm.add_pass(ModuleToFunctionPassAdaptor::new(fpm));
        let mut mam = ModuleAnalysisManager::new();
        let mut fam = FunctionAnalysisManager::new();

        mpm.run(m.verify()?, &mut mam, &mut fam)?;
        assert_eq!(
            &*names.borrow(),
            &["f".to_owned(), "g".to_owned(), "h".to_owned()]
        );
        Ok(())
    })
}

/// `llvmkit-specific subset` of `PassManagerTest.cpp` analysis query behavior:
/// a function pass can query `DominatorTreeAnalysis` and preserve it, but
/// llvmkit lacks the upstream module/function analysis proxy cache surface.
#[test]
fn function_pass_can_query_dominator_tree_analysis() -> Result<(), IrError> {
    with_sample_module(|m| {
        let f = m.function_by_name("f").expect("sample has f");
        let names = Rc::new(RefCell::new(Vec::new()));
        let mut fpm = FunctionPassManager::<_, PreservesVerification>::new_read_only();
        fpm.add_pass(RecordingFunctionPass {
            names,
            query_dt: true,
        });
        let mut mpm = ModulePassManager::<_, PreservesVerification>::new_read_only();
        mpm.add_pass(ModuleToFunctionPassAdaptor::new(fpm));
        let mut mam = ModuleAnalysisManager::new();
        let mut fam = FunctionAnalysisManager::new();
        fam.register_pass(DominatorTreeAnalysis);

        let _ = mpm.run(m.verify()?, &mut mam, &mut fam)?;
        assert!(
            fam.get_cached_result::<DominatorTreeAnalysis, _>(f)
                .is_some()
        );
        Ok(())
    })
}
