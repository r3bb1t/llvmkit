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

/// Prints how many callbacks are registered on each hook, never the
/// callbacks: they are `Box<dyn FnMut>`, which has no `Debug` and nothing
/// meaningful to show. The registry is shared through `Rc<RefCell<…>>`, so a
/// callback that is mid-fire holds the cell open — `try_borrow` keeps `Debug`
/// from panicking in exactly the situation a caller is most likely to print
/// one.
impl core::fmt::Debug for PassInstrumentationCallbacks {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut out = f.debug_struct("PassInstrumentationCallbacks");
        match self.storage.try_borrow() {
            Ok(storage) => out
                .field("before_pass", &storage.before_pass.len())
                .field("after_pass", &storage.after_pass.len())
                .field("before_analysis", &storage.before_analysis.len())
                .field("after_analysis", &storage.after_analysis.len()),
            Err(_) => out.field("callbacks", &"<firing>"),
        }
        .finish()
    }
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
