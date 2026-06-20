//! Pass instrumentation callback coverage.
//!
//! Every test cites its upstream source per Doctrine D11.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use llvmkit_ir::{
    DominatorTreeAnalysis, FunctionAnalysisManager, FunctionPassManager, IRBuilder, IrError,
    Linkage, Module, ModuleAnalysisManager, ModulePassManager, PassInstrumentationCallbacks,
    PreservedAnalyses, PreservesVerification, ReadOnlyFunctionPass, ReadOnlyFunctionPassContext,
    ReadOnlyModulePass, ReadOnlyModulePassContext,
};

fn with_sample_module<R, F>(run: F) -> Result<R, IrError>
where
    F: for<'ctx> FnOnce(Module<'ctx>) -> Result<R, IrError>,
{
    Module::with_new("pi", |m| {
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<()>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        IRBuilder::new_for::<()>(&m)
            .position_at_end(entry)
            .build_ret_void();
        run(m)
    })
}

struct NamedModulePass {
    ran: Rc<RefCell<Vec<&'static str>>>,
}

impl<'ctx> ReadOnlyModulePass<'ctx> for NamedModulePass {
    fn run(
        &mut self,
        _cx: &mut ReadOnlyModulePassContext<'_, 'ctx>,
    ) -> llvmkit_ir::IrResult<PreservedAnalyses> {
        self.ran.borrow_mut().push("module");
        Ok(PreservedAnalyses::all())
    }
}

struct RequiredFunctionPass {
    ran: Rc<Cell<u32>>,
}

impl<'ctx> ReadOnlyFunctionPass<'ctx> for RequiredFunctionPass {
    fn run(
        &mut self,
        _cx: &mut ReadOnlyFunctionPassContext<'_, 'ctx>,
    ) -> llvmkit_ir::IrResult<PreservedAnalyses> {
        self.ran.set(self.ran.get() + 1);
        Ok(PreservedAnalyses::all())
    }

    fn is_required(&self) -> bool {
        true
    }
}

/// `llvmkit-specific subset`: ports the optional-pass skip direction from
/// `unittests/IR/PassBuilderCallbacksTest.cpp` `InstrumentedSkippedPasses`.
/// llvmkit has no separate skipped/non-skipped callbacks, so the retained
/// assertion is the exact modeled `before` callback and absence of `after`.
#[test]
fn instrumentation_orders_and_skips_optional_passes() -> Result<(), IrError> {
    with_sample_module(|m| {
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

        let mut mpm = ModulePassManager::<_, PreservesVerification>::new_read_only();
        mpm.set_instrumentation(callbacks);
        mpm.add_pass(NamedModulePass { ran: ran.clone() });
        let mut mam = ModuleAnalysisManager::new();
        let mut fam = FunctionAnalysisManager::new();
        mpm.run(m.verify()?, &mut mam, &mut fam)?;

        assert!(ran.borrow().is_empty());
        let pass_name = std::any::type_name::<NamedModulePass>();
        let expected = vec![format!("before:{pass_name}:false")];
        assert_eq!(&*events.borrow(), &expected);
        Ok(())
    })
}

/// `llvmkit-specific subset`: ports LLVM's required-pass skip override.
/// llvmkit models the required flag on the `before` callback, but not the
/// separate upstream non-skipped callback.
#[test]
fn required_passes_cannot_be_skipped() -> Result<(), IrError> {
    with_sample_module(|m| {
        let ran = Rc::new(Cell::new(0));
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
            callbacks.register_after_pass_callback(move |name, pa| {
                events.borrow_mut().push(format!(
                    "after:{name}:{}",
                    pa.checker::<RequiredFunctionPass>().preserved()
                ));
            });
        }

        let mut fpm = FunctionPassManager::<_, PreservesVerification>::new_read_only();
        let pass_name = std::any::type_name::<RequiredFunctionPass>();
        fpm.set_instrumentation(callbacks.clone());
        fpm.add_pass(RequiredFunctionPass { ran: ran.clone() });
        let mut fam = FunctionAnalysisManager::new();
        let f = m.function_by_name("f").expect("sample has f");
        let verified = m.verify()?;
        let _ = fpm.run(verified, f, &mut fam)?;

        assert_eq!(ran.get(), 1);
        let expected = vec![
            format!("before:{pass_name}:true"),
            format!("after:{pass_name}:true"),
        ];
        assert_eq!(&*events.borrow(), &expected);
        Ok(())
    })
}

/// `llvmkit-specific subset`: ports the upstream before/after analysis
/// computation callbacks. Cached lookups are asserted not to fire callbacks.
#[test]
fn analysis_callbacks_fire_only_on_computation() -> Result<(), IrError> {
    with_sample_module(|m| {
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

        let analysis_name = std::any::type_name::<DominatorTreeAnalysis>();
        let expected = vec![
            format!("before-analysis:{analysis_name}"),
            format!("after-analysis:{analysis_name}"),
        ];
        assert_eq!(&*events.borrow(), &expected);
        Ok(())
    })
}
