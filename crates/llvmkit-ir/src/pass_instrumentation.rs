//! Pass and analysis instrumentation callbacks. This is the minimal
//! callback surface the analysis managers fire (`before_analysis` /
//! `after_analysis`); pass-level (`before_pass` / `after_pass`) firing
//! is registerable but not yet wired into the pass drivers.

use std::cell::RefCell;
use std::rc::Rc;

use crate::PreservedAnalyses;

type BeforePassCallback = Box<dyn FnMut(&str, bool) -> bool>;
type AfterPassCallback = Box<dyn FnMut(&str, &PreservedAnalyses)>;
type BeforeAnalysisCallback = Box<dyn FnMut(&str)>;
type AfterAnalysisCallback = Box<dyn FnMut(&str)>;

#[derive(Default)]
struct CallbackStorage {
    before_pass: Vec<BeforePassCallback>,
    after_pass: Vec<AfterPassCallback>,
    before_analysis: Vec<BeforeAnalysisCallback>,
    after_analysis: Vec<AfterAnalysisCallback>,
}

/// Shared callback registry. Clones point at the same callback vectors.
#[derive(Clone, Default)]
pub struct PassInstrumentationCallbacks {
    storage: Rc<RefCell<CallbackStorage>>,
}

impl PassInstrumentationCallbacks {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_before_pass_callback<F>(&self, callback: F)
    where
        F: FnMut(&str, bool) -> bool + 'static,
    {
        self.storage
            .borrow_mut()
            .before_pass
            .push(Box::new(callback));
    }

    pub fn register_after_pass_callback<F>(&self, callback: F)
    where
        F: FnMut(&str, &PreservedAnalyses) + 'static,
    {
        self.storage
            .borrow_mut()
            .after_pass
            .push(Box::new(callback));
    }

    pub fn register_before_analysis_callback<F>(&self, callback: F)
    where
        F: FnMut(&str) + 'static,
    {
        self.storage
            .borrow_mut()
            .before_analysis
            .push(Box::new(callback));
    }

    pub fn register_after_analysis_callback<F>(&self, callback: F)
    where
        F: FnMut(&str) + 'static,
    {
        self.storage
            .borrow_mut()
            .after_analysis
            .push(Box::new(callback));
    }

    pub(crate) fn run_before_analysis(&self, name: &str) {
        let mut callbacks = self.storage.borrow_mut();
        for callback in &mut callbacks.before_analysis {
            callback(name);
        }
    }

    pub(crate) fn run_after_analysis(&self, name: &str) {
        let mut callbacks = self.storage.borrow_mut();
        for callback in &mut callbacks.after_analysis {
            callback(name);
        }
    }
}

/// Analysis marker for retrieving instrumentation through an analysis manager
/// once the broader proxy layer exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PassInstrumentationAnalysis;
