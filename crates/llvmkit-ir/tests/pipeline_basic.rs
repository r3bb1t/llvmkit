//! Verdict-derivation locks for the typed tuple pipelines
//! (`function_pipeline`/`module_pipeline`/`for_each_function`).
//!
//! These re-establish the coverage the Task 3 cutover deleted along with the
//! old effect-typed `tests/typed_pipeline_basic.rs`, rewritten to the Pass API
//! v2 traits. Each test binds the pipeline's result with an EXPLICIT
//! `Module<'_, _, Verified>` / `Module<'_, _, Unverified>` annotation: that
//! annotation is the compile-time half of the lock — a wrong derived verdict
//! fails to compile — and the runtime asserts prove the passes genuinely ran,
//! in order, observing each other's effects and invalidations.
//!
//! D11 provenance: llvmkit-specific pipeline verdict-derivation lock (no upstream
//! analog — LLVM pipelines have no compile-time verified/unverified typestate;
//! the run/caching flow these wrap ports `unittests/IR/PassManagerTest.cpp`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use llvmkit_ir::{
    Analyses, DcePass, DominatorTreeAnalysis, Dyn, FnCx, FnReport, FunctionPass, FunctionView,
    IRBuilder, Inspect, IrError, IrResult, Linkage, ModCx, ModReport, Module, ModuleBrand,
    ModulePass, NoFolder, PatchBody, ReshapeCfg, RewriteModule, Unverified, Verified,
    for_each_function, function_pipeline, module_pipeline,
};

// ==========================================================================
// IR-builder fixtures (modelled on tests/scalar_cleanup_passes.rs)
// ==========================================================================

/// Single-block `i32 @<name>()` whose entry just returns a constant.
fn build_ret_i32_named<'ctx, B: ModuleBrand + 'ctx>(
    m: &Module<'ctx, B, Unverified>,
    name: &str,
) -> Result<FunctionView<'ctx, B>, IrError> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type_no_params(i32_ty, false);
    let f = m.add_function_dyn(name, fn_ty, Linkage::External)?;
    let entry = f.append_basic_block(m, "entry");
    let b = IRBuilder::new_for::<Dyn>(m).position_at_end(entry);
    b.build_ret(i32_ty.const_int(1_u32))?;
    Ok(f.into())
}

/// `i32 @<name>()` with one unused `add` named `dead` before the terminator.
/// Built with [`NoFolder`] so the constant add is not folded away, giving
/// `DcePass` a real trivially-dead instruction to erase.
fn build_dead_add_named<'ctx, B: ModuleBrand + 'ctx>(
    m: &Module<'ctx, B, Unverified>,
    name: &str,
) -> Result<FunctionView<'ctx, B>, IrError> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type_no_params(i32_ty, false);
    let f = m.add_function_dyn(name, fn_ty, Linkage::External)?;
    let entry = f.append_basic_block(m, "entry");
    let b = IRBuilder::with_folder(m, NoFolder).position_at_end(entry);
    let _dead = b.build_int_add::<i32, _, _, _>(
        i32_ty.const_int(10_u32),
        i32_ty.const_int(20_u32),
        "dead",
    )?;
    b.build_ret(i32_ty.const_int(1_u32))?;
    Ok(f.into())
}

// ==========================================================================
// Pass fixtures on the new FunctionPass / ModulePass traits
// ==========================================================================

/// Read-only [`Inspect`] function pass that appends a tag to a shared log, so a
/// passing test proves the member ran and in what order.
struct LogFnPass {
    log: Rc<RefCell<Vec<&'static str>>>,
    tag: &'static str,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for LogFnPass {
    type Access = Inspect;
    type Requires = ();
    const NAME: &'static str = "log-fn";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, Inspect, ()>) -> IrResult<FnReport> {
        self.log.borrow_mut().push(self.tag);
        Ok(cx.done())
    }
}

/// Read-only [`Inspect`] function pass that records its function's entry-block
/// instruction count, so a later member can prove it observed an earlier
/// member's erase.
struct ObserveEntryCount {
    seen: Rc<Cell<usize>>,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for ObserveEntryCount {
    type Access = Inspect;
    type Requires = ();
    const NAME: &'static str = "observe-count";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, Inspect, ()>) -> IrResult<FnReport> {
        let count = cx
            .function()
            .entry_block()
            .map_or(0, |bb| bb.instruction_count());
        self.seen.set(count);
        Ok(cx.done())
    }
}

/// Mutating [`PatchBody`] function pass that records the visited function's name
/// and enters the mutator (downgrading the module) without editing — enough to
/// prove `for_each_function` reached the definition and that the rung alone
/// drives the verdict.
struct LoggingMutator {
    visited: Rc<RefCell<Vec<String>>>,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for LoggingMutator {
    type Access = PatchBody;
    type Requires = ();
    const NAME: &'static str = "logging-mutator";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, PatchBody, ()>) -> IrResult<FnReport> {
        self.visited
            .borrow_mut()
            .push(cx.function().name().to_owned());
        let patch = cx.mutate();
        Ok(patch.done())
    }
}

/// Mutating [`ReshapeCfg`] function pass that enters and immediately finishes the
/// reshape mutator: its `done()` reports the `none()` floor, so every function
/// analysis (including the CFG-shaped [`DominatorTreeAnalysis`]) is invalidated.
struct NoOpReshape;

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for NoOpReshape {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "noop-reshape";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        Ok(reshape.done())
    }
}

/// A `ReshapeCfg` pass that actually mutates: it erases every dead
/// (use-less, non-terminator) instruction, so the mutator's dirty flag is
/// set and `done()` reports the `none()` floor (invalidating everything).
struct MutatingReshape;

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for MutatingReshape {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "mutating-reshape";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let dead: Vec<_> = reshape
            .function()
            .basic_blocks()
            .flat_map(|block| block.instructions().collect::<Vec<_>>())
            .filter(|view| !view.is_terminator() && !view.to_erased().has_uses())
            .collect();
        for view in &dead {
            reshape.erase(
                &view
                    .as_non_terminator()
                    .expect("filtered to non-terminators"),
            );
        }
        Ok(reshape.done())
    }
}

/// Read-only [`Inspect`] module pass that flips a flag once it has seen the
/// module's functions.
struct CountFunctionsPass {
    ran: Rc<Cell<bool>>,
}

impl<'ctx, B: ModuleBrand + 'ctx> ModulePass<'ctx, B> for CountFunctionsPass {
    type Access = Inspect;
    type Requires = ();
    const NAME: &'static str = "count-functions";

    fn run(&mut self, cx: ModCx<'_, '_, '_, 'ctx, B, Inspect, ()>) -> IrResult<ModReport> {
        assert!(
            cx.functions().count() >= 1,
            "module pass must see a function"
        );
        self.ran.set(true);
        Ok(cx.done())
    }
}

/// Mutating [`RewriteModule`] module pass that adds a global through the raw
/// module token and flips a flag.
struct AddGlobalPass {
    ran: Rc<Cell<bool>>,
}

impl<'ctx, B: ModuleBrand + 'ctx> ModulePass<'ctx, B> for AddGlobalPass {
    type Access = RewriteModule;
    type Requires = ();
    const NAME: &'static str = "add-global";

    fn run(&mut self, cx: ModCx<'_, '_, '_, 'ctx, B, RewriteModule, ()>) -> IrResult<ModReport> {
        let rewrite = cx.mutate();
        let i32_ty = rewrite.module_mut().i32_type();
        rewrite.module_mut().add_global("g", i32_ty.const_zero())?;
        self.ran.set(true);
        Ok(rewrite.done())
    }
}

// ==========================================================================
// 1. all-Inspect function pipeline ⇒ Verified
// ==========================================================================

#[test]
fn all_inspect_function_pipeline_stays_verified() -> Result<(), IrError> {
    Module::with_new("pipe-fn-ro", |m| {
        let f = build_ret_i32_named(&m, "f")?;
        let verified = m.verify()?;
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut pipe = function_pipeline((
            LogFnPass {
                log: log.clone(),
                tag: "a",
            },
            LogFnPass {
                log: log.clone(),
                tag: "b",
            },
        ));
        let mut analyses = Analyses::new();
        // The explicit `Verified` annotation is the compile-time half: an
        // all-`Inspect` pipeline folds to `StaysVerified`.
        let _still_verified: Module<'_, _, Verified> = pipe.run(verified, f, &mut analyses)?;
        assert_eq!(*log.borrow(), vec!["a", "b"]);
        Ok(())
    })
}

// ==========================================================================
// 2. any-mutator function pipeline ⇒ Unverified
// ==========================================================================

#[test]
fn function_pipeline_with_mutator_downgrades() -> Result<(), IrError> {
    Module::with_new("pipe-fn-mixed", |m| {
        let f = build_dead_add_named(&m, "f")?;
        let verified = m.verify()?;
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut pipe = function_pipeline((
            LogFnPass {
                log: log.clone(),
                tag: "ro",
            },
            DcePass,
        ));
        let mut analyses = Analyses::new();
        // `DcePass` is `PatchBody`, so the folded verdict is `Downgrades`: this
        // only type-checks because the mutator downgraded the output typestate.
        let unverified: Module<'_, _, Unverified> = pipe.run(verified, f, &mut analyses)?;
        let reverified = unverified.verify()?;
        assert_eq!(*log.borrow(), vec!["ro"]);
        assert!(!format!("{reverified}").contains("%dead"));
        Ok(())
    })
}

// ==========================================================================
// 3. module pipeline: all-Inspect ⇒ Verified; RewriteModule member ⇒ Unverified
// ==========================================================================

#[test]
fn module_pipeline_all_inspect_stays_verified() -> Result<(), IrError> {
    Module::with_new("pipe-mod-ro", |m| {
        let _f = build_ret_i32_named(&m, "f")?;
        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let ran = Rc::new(Cell::new(false));
        let mut pipe = module_pipeline((CountFunctionsPass { ran: ran.clone() },));
        let _still_verified: Module<'_, _, Verified> = pipe.run(verified, &mut analyses)?;
        assert!(ran.get(), "Inspect module pass must run");
        Ok(())
    })
}

#[test]
fn module_pipeline_with_rewrite_downgrades() -> Result<(), IrError> {
    Module::with_new("pipe-mod-rewrite", |m| {
        let _f = build_ret_i32_named(&m, "f")?;
        let verified = m.verify()?;
        assert_eq!(verified.globals().len(), 0);
        let mut analyses = Analyses::new();
        let ran = Rc::new(Cell::new(false));
        let mut pipe = module_pipeline((AddGlobalPass { ran: ran.clone() },));
        // `RewriteModule` folds to `Downgrades`.
        let unverified: Module<'_, _, Unverified> = pipe.run(verified, &mut analyses)?;
        assert!(ran.get(), "RewriteModule pass must run");
        assert_eq!(unverified.globals().len(), 1);
        unverified.verify()?;
        Ok(())
    })
}

// ==========================================================================
// 4. for_each_function: read-only stays Verified; mutating downgrades and
//    visits every DEFINITION (skipping declarations)
// ==========================================================================

#[test]
fn for_each_function_read_only_stays_verified() -> Result<(), IrError> {
    Module::with_new("pipe-foreach-ro", |m| {
        let _f = build_ret_i32_named(&m, "f")?;
        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut pipe = module_pipeline((for_each_function(function_pipeline((LogFnPass {
            log: log.clone(),
            tag: "visited",
        },))),));
        // The wrapped function pipeline is all read-only, so its verdict
        // propagates out as `StaysVerified`.
        let _still_verified: Module<'_, _, Verified> = pipe.run(verified, &mut analyses)?;
        assert_eq!(*log.borrow(), vec!["visited"]);
        Ok(())
    })
}

#[test]
fn for_each_function_mutating_downgrades_and_visits_defs() -> Result<(), IrError> {
    Module::with_new("pipe-foreach-mut", |m| {
        let f1 = build_dead_add_named(&m, "f1")?;
        let f2 = build_dead_add_named(&m, "f2")?;
        // A declaration (no body) — must be skipped by `for_each_function`.
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let _decl = m.add_function_dyn("ext", fn_ty, Linkage::External)?;

        assert_eq!(f1.entry_block().expect("def").instruction_count(), 2);
        assert_eq!(f2.entry_block().expect("def").instruction_count(), 2);

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let visited = Rc::new(RefCell::new(Vec::new()));
        let mut pipe = module_pipeline((for_each_function(function_pipeline((
            LoggingMutator {
                visited: visited.clone(),
            },
            DcePass,
        ))),));
        // The inner function pipeline mutates (`PatchBody`), so its `Downgrades`
        // verdict propagates out through `for_each_function` to the module.
        let unverified: Module<'_, _, Unverified> = pipe.run(verified, &mut analyses)?;
        let reverified = unverified.verify()?;
        // Both definitions were visited in module order; the declaration was not.
        assert_eq!(*visited.borrow(), vec!["f1".to_owned(), "f2".to_owned()]);
        // `DcePass` really erased the dead add in each definition.
        assert!(!format!("{reverified}").contains("%dead"));
        Ok(())
    })
}

// ==========================================================================
// 5. nested pipeline composes and its verdict propagates
// ==========================================================================

#[test]
fn nested_read_only_pipeline_stays_verified() -> Result<(), IrError> {
    Module::with_new("pipe-nested-ro", |m| {
        let f = build_ret_i32_named(&m, "f")?;
        let verified = m.verify()?;
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut pipe = function_pipeline((
            LogFnPass {
                log: log.clone(),
                tag: "outer",
            },
            function_pipeline((
                LogFnPass {
                    log: log.clone(),
                    tag: "inner-a",
                },
                LogFnPass {
                    log: log.clone(),
                    tag: "inner-b",
                },
            )),
        ));
        let mut analyses = Analyses::new();
        // A nested all-read-only pipeline folds to `StaysVerified`, and that
        // folded verdict joins cleanly with the outer member.
        let _still_verified: Module<'_, _, Verified> = pipe.run(verified, f, &mut analyses)?;
        assert_eq!(*log.borrow(), vec!["outer", "inner-a", "inner-b"]);
        Ok(())
    })
}

#[test]
fn nested_pipeline_with_inner_mutator_downgrades() -> Result<(), IrError> {
    Module::with_new("pipe-nested-mixed", |m| {
        let f = build_dead_add_named(&m, "f")?;
        let verified = m.verify()?;
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut pipe = function_pipeline((
            LogFnPass {
                log: log.clone(),
                tag: "outer",
            },
            function_pipeline((
                LogFnPass {
                    log: log.clone(),
                    tag: "inner-ro",
                },
                DcePass,
            )),
        ));
        let mut analyses = Analyses::new();
        // The inner pipeline's own folded verdict is `Downgrades`; that — not the
        // leaf members — is what the outer pipeline joins against, downgrading the
        // whole run (D8).
        let unverified: Module<'_, _, Unverified> = pipe.run(verified, f, &mut analyses)?;
        let reverified = unverified.verify()?;
        assert_eq!(*log.borrow(), vec!["outer", "inner-ro"]);
        assert!(!format!("{reverified}").contains("%dead"));
        Ok(())
    })
}

// ==========================================================================
// 6. ordering + invalidation
// ==========================================================================

#[test]
fn pipeline_runs_in_order_and_second_member_sees_first() -> Result<(), IrError> {
    Module::with_new("pipe-order", |m| {
        let f = build_dead_add_named(&m, "f")?;
        // Entry starts with `dead` + `ret`.
        assert_eq!(f.entry_block().expect("def").instruction_count(), 2);
        let verified = m.verify()?;
        let seen = Rc::new(Cell::new(0usize));
        // Member 1 (`DcePass`, PatchBody) erases the dead add; member 2
        // (`ObserveEntryCount`, Inspect) then observes the shrunken block.
        let mut pipe = function_pipeline((DcePass, ObserveEntryCount { seen: seen.clone() }));
        let mut analyses = Analyses::new();
        let unverified: Module<'_, _, Unverified> = pipe.run(verified, f, &mut analyses)?;
        // Member 2 saw only `ret` — proving member 1 ran first and its effect
        // was visible.
        assert_eq!(seen.get(), 1);
        unverified.verify()?;
        Ok(())
    })
}

#[test]
fn mutating_member_invalidates_and_analysis_recomputes() -> Result<(), IrError> {
    Module::with_new("pipe-invalidate", |m| {
        let f = build_dead_add_named(&m, "f")?;
        let verified = m.verify()?;
        let mut analyses = Analyses::new();

        // Register and compute the dominator tree, then confirm it is cached.
        analyses.register_function_analysis(DominatorTreeAnalysis);
        let _dt = analyses
            .function_manager_mut()
            .get_result::<DominatorTreeAnalysis, _>(f)?;
        assert!(
            analyses
                .function_manager()
                .get_cached_result::<DominatorTreeAnalysis, _>(f)
                .is_some(),
            "dominator tree must be cached after computing it"
        );

        // A witnessed no-op `ReshapeCfg` run preserves everything: its dirty
        // flag saw no mutation, so `done()` reports all-preserved and the
        // cached dominator tree survives — no needless invalidation.
        let mut noop = function_pipeline((NoOpReshape,));
        let after_noop: Module<'_, _, Unverified> = noop.run(verified, f, &mut analyses)?;
        assert!(
            analyses
                .function_manager()
                .get_cached_result::<DominatorTreeAnalysis, _>(f)
                .is_some(),
            "a witnessed no-op ReshapeCfg run must preserve the cached dominator tree"
        );

        // A `ReshapeCfg` pass that actually erases an instruction sets the
        // dirty flag, so its `done()` reports the `none()` floor and the
        // pipeline invalidates the (non-preserved) dominator tree.
        let reverified = after_noop.verify()?;
        let mut pipe = function_pipeline((MutatingReshape,));
        let unverified: Module<'_, _, Unverified> = pipe.run(reverified, f, &mut analyses)?;

        // The cached tree is gone after the mutating member's invalidation.
        assert!(
            analyses
                .function_manager()
                .get_cached_result::<DominatorTreeAnalysis, _>(f)
                .is_none(),
            "a mutating ReshapeCfg run's none() floor must invalidate the cached dominator tree"
        );

        // The still-registered analysis recomputes on demand.
        let dt = analyses
            .function_manager_mut()
            .get_result::<DominatorTreeAnalysis, _>(f)?;
        let entry = f.entry_block().expect("definition has an entry block");
        assert!(dt.is_reachable_from_entry(entry));
        assert!(
            analyses
                .function_manager()
                .get_cached_result::<DominatorTreeAnalysis, _>(f)
                .is_some(),
            "dominator tree must be re-cached after recomputation"
        );

        unverified.verify()?;
        Ok(())
    })
}
