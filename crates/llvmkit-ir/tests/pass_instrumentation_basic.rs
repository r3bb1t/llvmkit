//! Pass instrumentation callback coverage.
//!
//! Every test cites its upstream source per Doctrine D11.

use std::cell::RefCell;
use std::rc::Rc;

use llvmkit_ir::{
    DominatorTreeAnalysis, FunctionAnalysisManager, FunctionPass, FunctionPassManager, IRBuilder,
    IrError, Linkage, Module, ModuleAnalysisManager, ModulePass, ModulePassManager,
    PassInstrumentationCallbacks, PreservedAnalyses,
};

fn sample_module() -> Result<Module<'static>, IrError> {
    let m = Module::new("pi");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    IRBuilder::new_for::<()>(&m)
        .position_at_end(entry)
        .build_ret_void();
    Ok(m)
}

struct NamedModulePass {
    ran: Rc<RefCell<Vec<&'static str>>>,
}

impl<'ctx> ModulePass<'ctx> for NamedModulePass {
    fn run(
        &mut self,
        _module: &'ctx Module<'ctx>,
        _mam: &mut ModuleAnalysisManager<'ctx>,
        _fam: &mut FunctionAnalysisManager<'ctx>,
    ) -> llvmkit_ir::IrResult<PreservedAnalyses> {
        self.ran.borrow_mut().push("module");
        Ok(PreservedAnalyses::all())
    }
}

struct RequiredFunctionPass;

impl<'ctx> FunctionPass<'ctx> for RequiredFunctionPass {
    fn run(
        &mut self,
        _function: llvmkit_ir::FunctionValue<'ctx, llvmkit_ir::Dyn>,
        _fam: &mut FunctionAnalysisManager<'ctx>,
    ) -> llvmkit_ir::IrResult<PreservedAnalyses> {
        Ok(PreservedAnalyses::all())
    }

    fn is_required(&self) -> bool {
        true
    }
}

/// Ports `unittests/IR/PassBuilderCallbacksTest.cpp`: before-pass callbacks
/// run before a pass, after-pass callbacks receive its `PreservedAnalyses`, and
/// returning `false` from before-pass skips optional passes.
#[test]
fn instrumentation_orders_and_skips_optional_passes() -> Result<(), IrError> {
    let m = sample_module()?;
    let events = Rc::new(RefCell::new(Vec::new()));
    let ran = Rc::new(RefCell::new(Vec::new()));
    let callbacks = PassInstrumentationCallbacks::new();
    {
        let events = events.clone();
        callbacks.register_before_pass_callback(move |name, required| {
            events
                .borrow_mut()
                .push(format!("before:{name}:{required}"));
            false
        });
    }
    {
        let events = events.clone();
        callbacks.register_after_pass_callback(move |name, pa| {
            events.borrow_mut().push(format!(
                "after:{name}:{}",
                pa.checker::<NamedModulePass>().preserved()
            ));
        });
    }

    let mut mpm = ModulePassManager::new();
    mpm.set_instrumentation(callbacks);
    mpm.add_pass(NamedModulePass { ran: ran.clone() });
    let mut mam = ModuleAnalysisManager::new();
    let mut fam = FunctionAnalysisManager::new();
    mpm.run(&m, &mut mam, &mut fam)?;

    assert!(ran.borrow().is_empty());
    assert!(events.borrow().iter().any(|e| e.starts_with("before:")));
    assert!(!events.borrow().iter().any(|e| e.starts_with("after:")));
    Ok(())
}

/// Ports `unittests/IR/PassBuilderCallbacksTest.cpp`: required passes cannot
/// be skipped even when a before-pass callback asks to skip them.
#[test]
fn required_passes_cannot_be_skipped() -> Result<(), IrError> {
    let m = sample_module()?;
    let events = Rc::new(RefCell::new(Vec::new()));
    let callbacks = PassInstrumentationCallbacks::new();
    {
        let events = events.clone();
        callbacks.register_before_pass_callback(move |name, required| {
            events
                .borrow_mut()
                .push(format!("before:{name}:{required}"));
            false
        });
    }
    {
        let events = events.clone();
        callbacks.register_after_pass_callback(move |name, _pa| {
            events.borrow_mut().push(format!("after:{name}"));
        });
    }

    let mut fpm = FunctionPassManager::new();
    fpm.set_instrumentation(callbacks.clone());
    fpm.add_pass(RequiredFunctionPass);
    let mut fam = FunctionAnalysisManager::new();
    let f = m.function_by_name("f").expect("sample has f");
    fpm.run(f, &mut fam)?;

    assert!(events.borrow().iter().any(|e| e.contains(":true")));
    assert!(events.borrow().iter().any(|e| e.starts_with("after:")));
    Ok(())
}

/// Ports `unittests/IR/PassBuilderCallbacksTest.cpp`: analysis callbacks fire
/// around a real analysis computation, not around cached lookups.
#[test]
fn analysis_callbacks_fire_only_on_computation() -> Result<(), IrError> {
    let m = sample_module()?;
    let f = m.function_by_name("f").expect("sample has f");
    let events = Rc::new(RefCell::new(Vec::new()));
    let callbacks = PassInstrumentationCallbacks::new();
    {
        let events = events.clone();
        callbacks.register_before_analysis_callback(move |name| {
            events.borrow_mut().push(format!("before-analysis:{name}"));
        });
    }
    {
        let events = events.clone();
        callbacks.register_after_analysis_callback(move |name| {
            events.borrow_mut().push(format!("after-analysis:{name}"));
        });
    }

    let mut fam = FunctionAnalysisManager::new();
    fam.set_instrumentation(callbacks);
    fam.register_pass(DominatorTreeAnalysis);
    let _ = fam.get_result::<DominatorTreeAnalysis>(f)?;
    let _ = fam.get_result::<DominatorTreeAnalysis>(f)?;

    let borrowed = events.borrow();
    assert_eq!(
        borrowed
            .iter()
            .filter(|e| e.starts_with("before-analysis:"))
            .count(),
        1
    );
    assert_eq!(
        borrowed
            .iter()
            .filter(|e| e.starts_with("after-analysis:"))
            .count(),
        1
    );
    Ok(())
}
