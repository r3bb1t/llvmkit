//! Runtime-composition (`Dyn…`) pass-pipeline locks — the opt-style escape hatch
//! (Doctrine D3) built over the crate-private erased pass trait.
//!
//! The tuple pipelines (`tests/pipeline_basic.rs`) fix their length and members at
//! compile time. These pipelines are assembled AT RUNTIME — passes are boxed and
//! `push`ed into a `Vec`, so the length is not known until the program runs. What
//! survives that erasure is the verified/unverified OUTPUT typestate: a transform
//! container always yields `Module<Unverified>`, a read-only container always
//! yields `Module<Verified>`, and — crucially — a mutating pass cannot even be
//! `push`ed into a read-only container (its rung does not implement the read-only
//! bound). Each test binds the result with an EXPLICIT `Module<'_, _, Verified>` /
//! `Module<'_, _, Unverified>` annotation (the compile-time half of the lock) and
//! asserts the passes genuinely ran, in push order, observing each other's effects.
//!
//! D11 provenance: llvmkit-specific runtime-composition escape-hatch lock (no
//! upstream analog: LLVM's `opt` assembles `Box<Pass>` pipelines with no
//! compile-time verified/unverified typestate).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use llvmkit_ir::{
    Analyses, DCE, DcePass, DynFunctionPipeline, DynModulePipeline, DynReadOnlyFunctionPipeline,
    DynReadOnlyModulePipeline, FnCx, FnReport, FunctionPass, FunctionView, IRBuilder, Inspect,
    IrError, IrResult, Linkage, ModCx, ModReport, Module, ModuleBrand, ModulePass, NoFolder,
    RewriteModule, Unverified, Verified,
};

// ==========================================================================
// IR-builder fixtures (shared shape with tests/pipeline_basic.rs)
// ==========================================================================

/// Single-block `i32 @<name>()` whose entry just returns a constant.
fn build_ret_i32_named<'ctx, B: ModuleBrand + 'ctx>(
    m: &Module<'ctx, B, Unverified>,
    name: &str,
) -> Result<FunctionView<'ctx, B>, IrError> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type_no_params(i32_ty, false);
    let f = m.add_function::<i32, _>(name, fn_ty, Linkage::External)?;
    let entry = f.append_basic_block(m, "entry");
    let b = IRBuilder::new_for::<i32>(m).position_at_end(entry);
    b.build_ret(i32_ty.const_int(1_u32))?;
    Ok(f.into())
}

/// `i32 @<name>()` with one unused `add` named `dead` before the terminator, built
/// with [`NoFolder`] so the constant add survives for `DcePass` to erase.
fn build_dead_add_named<'ctx, B: ModuleBrand + 'ctx>(
    m: &Module<'ctx, B, Unverified>,
    name: &str,
) -> Result<FunctionView<'ctx, B>, IrError> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type_no_params(i32_ty, false);
    let f = m.add_function::<i32, _>(name, fn_ty, Linkage::External)?;
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
// Pass fixtures (FunctionPass / ModulePass)
// ==========================================================================

/// Read-only [`Inspect`] function pass that appends a tag to a shared log.
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
/// instruction count, so a later member can prove it observed an earlier erase.
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

/// Read-only [`Inspect`] module pass that counts the module's functions and flips
/// a shared flag.
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

/// Mutating [`RewriteModule`] module pass that adds a global through the raw module
/// token and flips a shared flag.
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

/// Mutating [`RewriteModule`] module pass that adds a UNIQUELY-named global, so
/// several instances can run in one pipeline without a name clash.
struct AddNamedGlobalPass {
    name: &'static str,
    ran: Rc<Cell<bool>>,
}

impl<'ctx, B: ModuleBrand + 'ctx> ModulePass<'ctx, B> for AddNamedGlobalPass {
    type Access = RewriteModule;
    type Requires = ();
    const NAME: &'static str = "add-named-global";

    fn run(&mut self, cx: ModCx<'_, '_, '_, 'ctx, B, RewriteModule, ()>) -> IrResult<ModReport> {
        let rewrite = cx.mutate();
        let i32_ty = rewrite.module_mut().i32_type();
        rewrite
            .module_mut()
            .add_global(self.name, i32_ty.const_zero())?;
        self.ran.set(true);
        Ok(rewrite.done())
    }
}

// ==========================================================================
// 1. transform Dyn function pipeline ⇒ Unverified + mutation + push order
// ==========================================================================

#[test]
fn transform_dyn_function_pipeline_downgrades_mutates_and_orders() -> Result<(), IrError> {
    Module::with_new("dyn-fn-transform", |m| {
        let f = build_dead_add_named(&m, "f")?;
        // Entry starts with `dead` + `ret`.
        assert_eq!(f.entry_block().expect("def").instruction_count(), 2);
        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        let seen = Rc::new(Cell::new(0usize));

        // Assemble a MIXED pipeline (read-only + mutating) by pushing boxed passes.
        let mut pipe = DynFunctionPipeline::new();
        pipe.push(LogFnPass {
            log: log.clone(),
            tag: "pre",
        });
        pipe.push(DcePass);
        pipe.push(ObserveEntryCount { seen: seen.clone() });

        // Push order is preserved in the boxed vec.
        let names: Vec<&str> = pipe.pass_names().collect();
        assert_eq!(names, vec!["log-fn", DCE.as_str(), "observe-count"]);
        assert!(!pipe.has_required_pass());

        // A transform container ALWAYS yields `Unverified` — the explicit
        // annotation is the compile-time half of the lock (a `DcePass` member
        // means at least one mutating rung; the container downgrades regardless).
        let unverified: Module<'_, _, Unverified> = pipe.run(verified, f, &mut analyses)?;
        let reverified = unverified.verify()?;

        // Member 1 ran; members 2/3 don't log.
        assert_eq!(*log.borrow(), vec!["pre"]);
        // Member 3 saw only `ret` — proving member 2 (`DcePass`) ran first and its
        // erase was visible to a later member (order + real mutation).
        assert_eq!(
            seen.get(),
            1,
            "observer must see only `ret` after DcePass erased the dead add"
        );
        assert!(!format!("{reverified}").contains("%dead"));
        Ok(())
    })
}

// ==========================================================================
// 2. read-only Dyn function pipeline ⇒ Verified + every member ran
// ==========================================================================

#[test]
fn read_only_dyn_function_pipeline_stays_verified_and_runs() -> Result<(), IrError> {
    Module::with_new("dyn-fn-readonly", |m| {
        let f = build_ret_i32_named(&m, "f")?;
        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let log = Rc::new(RefCell::new(Vec::new()));

        // Only `Inspect` passes are admissible — a mutating pass would not compile
        // at `push` (that missing bound is the type-level `Verified` guarantee).
        let mut pipe = DynReadOnlyFunctionPipeline::new();
        pipe.push(LogFnPass {
            log: log.clone(),
            tag: "a",
        });
        pipe.push(LogFnPass {
            log: log.clone(),
            tag: "b",
        });

        // A read-only container ALWAYS yields `Verified`, threading the original
        // module through untouched — the explicit annotation is the lock.
        let still_verified: Module<'_, _, Verified> = pipe.run(verified, f, &mut analyses)?;

        assert_eq!(*log.borrow(), vec!["a", "b"]);
        // Read-only: the IR is untouched.
        assert!(format!("{still_verified}").contains("ret i32 1"));
        Ok(())
    })
}

// ==========================================================================
// 3. Dyn module pipeline: transform ⇒ Unverified + mutation; read-only ⇒ Verified
// ==========================================================================

#[test]
fn transform_dyn_module_pipeline_downgrades_and_mutates() -> Result<(), IrError> {
    Module::with_new("dyn-mod-transform", |m| {
        let _f = build_ret_i32_named(&m, "f")?;
        let verified = m.verify()?;
        assert_eq!(verified.iter_globals().len(), 0);
        let mut analyses = Analyses::new();
        let ran = Rc::new(Cell::new(false));

        let mut pipe = DynModulePipeline::new();
        pipe.push(AddGlobalPass { ran: ran.clone() });

        // A `RewriteModule` member downgrades the module; the transform container's
        // output is unconditionally `Unverified`.
        let unverified: Module<'_, _, Unverified> = pipe.run(verified, &mut analyses)?;

        assert!(ran.get(), "RewriteModule pass must run");
        // The real mutation landed on the returned module.
        assert_eq!(unverified.iter_globals().len(), 1);
        unverified.verify()?;
        Ok(())
    })
}

#[test]
fn read_only_dyn_module_pipeline_stays_verified_and_runs() -> Result<(), IrError> {
    Module::with_new("dyn-mod-readonly", |m| {
        let _f = build_ret_i32_named(&m, "f")?;
        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let ran = Rc::new(Cell::new(false));

        let mut pipe = DynReadOnlyModulePipeline::new();
        pipe.push(CountFunctionsPass { ran: ran.clone() });

        let still_verified: Module<'_, _, Verified> = pipe.run(verified, &mut analyses)?;

        assert!(ran.get(), "Inspect module pass must run");
        assert_eq!(still_verified.as_view().iter_functions().count(), 1);
        Ok(())
    })
}

// ==========================================================================
// 4. runtime assembly: variable-length pipeline a tuple cannot express
// ==========================================================================

#[test]
fn runtime_assembly_variable_length_pipeline() -> Result<(), IrError> {
    Module::with_new("dyn-fn-runtime-assembly", |m| {
        let f = build_ret_i32_named(&m, "f")?;
        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let log = Rc::new(RefCell::new(Vec::new()));

        // The count comes from a runtime value: each distinct tuple arity is a
        // DISTINCT type, so no single tuple pipeline can be this length-generic.
        let tags: Vec<&'static str> = vec!["p0", "p1", "p2", "p3", "p4"];
        let mut pipe = DynReadOnlyFunctionPipeline::new();
        for &tag in &tags {
            pipe.push(LogFnPass {
                log: log.clone(),
                tag,
            });
        }
        assert_eq!(pipe.len(), tags.len());
        assert_eq!(pipe.pass_names().count(), tags.len());

        let still_verified: Module<'_, _, Verified> = pipe.run(verified, f, &mut analyses)?;

        // Every pushed pass ran, in push order.
        assert_eq!(*log.borrow(), tags);
        assert_eq!(still_verified.as_view().iter_functions().count(), 1);
        Ok(())
    })
}

// ==========================================================================
// 5. runtime assembly at module level: transform, variable length
// ==========================================================================

#[test]
fn runtime_assembly_module_transform_variable_length() -> Result<(), IrError> {
    Module::with_new("dyn-mod-runtime-assembly", |m| {
        let _f = build_ret_i32_named(&m, "f")?;
        let verified = m.verify()?;
        let mut analyses = Analyses::new();

        // Push a runtime-chosen number of `RewriteModule` passes; each adds one
        // uniquely-named global. The pipeline length is a runtime `count`, not a
        // tuple arity.
        let names: Vec<&'static str> = vec!["g0", "g1", "g2"];
        let count = names.len();
        let flags: Vec<Rc<Cell<bool>>> = (0..count).map(|_| Rc::new(Cell::new(false))).collect();
        let mut pipe = DynModulePipeline::new();
        for (name, flag) in names.iter().zip(&flags) {
            pipe.push(AddNamedGlobalPass {
                name,
                ran: flag.clone(),
            });
        }
        assert_eq!(pipe.len(), count);

        let unverified: Module<'_, _, Unverified> = pipe.run(verified, &mut analyses)?;

        assert!(flags.iter().all(|f| f.get()), "every pushed pass must run");
        // Each `AddGlobalPass` inserts a global named "g"; three members ⇒ three.
        assert_eq!(unverified.iter_globals().len(), count);
        Ok(())
    })
}
