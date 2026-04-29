//! Minimal module/function pass-manager coverage.
//!
//! Every test cites its upstream source per Doctrine D11.

use std::cell::RefCell;
use std::rc::Rc;

use llvmkit_ir::{
    DominatorTreeAnalysis, FunctionAnalysisManager, FunctionPass, FunctionPassManager, IRBuilder,
    IrError, Linkage, Module, ModuleAnalysisManager, ModulePass, ModulePassManager,
    ModuleToFunctionPassAdaptor, PreservedAnalyses,
};

fn sample_module() -> Result<Module<'static>, IrError> {
    let m = Module::new("pm");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    for name in ["f", "g"] {
        let f = m.add_function::<()>(name, fn_ty, Linkage::External)?;
        let entry = f.append_basic_block("entry");
        IRBuilder::new_for::<()>(&m)
            .position_at_end(entry)
            .build_ret_void();
    }
    let _decl = m.add_function::<()>("decl", fn_ty, Linkage::External)?;
    Ok(m)
}

struct RecordingModulePass {
    name: &'static str,
    order: Rc<RefCell<Vec<&'static str>>>,
    preserved: PreservedAnalyses,
}

impl<'ctx> ModulePass<'ctx> for RecordingModulePass {
    fn run(
        &mut self,
        _module: &'ctx Module<'ctx>,
        _mam: &mut ModuleAnalysisManager<'ctx>,
        _fam: &mut FunctionAnalysisManager<'ctx>,
    ) -> llvmkit_ir::IrResult<PreservedAnalyses> {
        self.order.borrow_mut().push(self.name);
        Ok(self.preserved.clone())
    }
}

struct RecordingFunctionPass {
    names: Rc<RefCell<Vec<String>>>,
    query_dt: bool,
}

impl<'ctx> FunctionPass<'ctx> for RecordingFunctionPass {
    fn run(
        &mut self,
        function: llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn>,
        fam: &mut FunctionAnalysisManager<'ctx>,
    ) -> llvmkit_ir::IrResult<PreservedAnalyses> {
        self.names.borrow_mut().push(function.name().to_owned());
        if self.query_dt {
            let dt = fam.get_result::<DominatorTreeAnalysis>(function)?;
            assert!(dt.is_reachable_from_entry(function.entry_block().expect("definition")));
        }
        Ok(PreservedAnalyses::all())
    }
}

/// Ports `unittests/IR/PassManagerTest.cpp::TEST_F(PassManagerTest, Basic)`:
/// module passes run in insertion order and their `PreservedAnalyses` drive invalidation.
#[test]
fn module_pass_manager_runs_in_order() -> Result<(), IrError> {
    let m = sample_module()?;
    let order = Rc::new(RefCell::new(Vec::new()));
    let mut mpm = ModulePassManager::new();
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

    let pa = mpm.run(&m, &mut mam, &mut fam)?;
    assert_eq!(&*order.borrow(), &["a", "b"]);
    assert!(!pa.checker::<DominatorTreeAnalysis>().preserved());
    Ok(())
}

/// Ports `unittests/IR/PassManagerTest.cpp` function-pass manager behavior:
/// the module-to-function adaptor runs function passes over definitions only.
#[test]
fn module_to_function_adaptor_runs_defined_functions_only() -> Result<(), IrError> {
    let m = sample_module()?;
    let names = Rc::new(RefCell::new(Vec::new()));
    let mut fpm = FunctionPassManager::new();
    fpm.add_pass(RecordingFunctionPass {
        names: names.clone(),
        query_dt: false,
    });
    let mut mpm = ModulePassManager::new();
    mpm.add_pass(ModuleToFunctionPassAdaptor::new(fpm));
    let mut mam = ModuleAnalysisManager::new();
    let mut fam = FunctionAnalysisManager::new();

    mpm.run(&m, &mut mam, &mut fam)?;
    assert_eq!(&*names.borrow(), &["f".to_owned(), "g".to_owned()]);
    Ok(())
}

/// Ports `unittests/IR/PassManagerTest.cpp` analysis-preservation behavior:
/// preserving all keeps `DominatorTreeAnalysis` cached across passes.
#[test]
fn function_pass_can_query_dominator_tree_analysis() -> Result<(), IrError> {
    let m = sample_module()?;
    let names = Rc::new(RefCell::new(Vec::new()));
    let mut fpm = FunctionPassManager::new();
    fpm.add_pass(RecordingFunctionPass {
        names,
        query_dt: true,
    });
    let mut mpm = ModulePassManager::new();
    mpm.add_pass(ModuleToFunctionPassAdaptor::new(fpm));
    let mut mam = ModuleAnalysisManager::new();
    let mut fam = FunctionAnalysisManager::new();
    fam.register_pass(DominatorTreeAnalysis);

    let pa = mpm.run(&m, &mut mam, &mut fam)?;
    assert!(pa.checker::<DominatorTreeAnalysis>().preserved());
    Ok(())
}
