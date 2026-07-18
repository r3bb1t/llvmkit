//! Behavioral parity locks for the `#[function_pass]` / `#[module_pass]`
//! attribute macros.
//!
//! Each macro-authored pass is paired with a hand-written twin implementing the
//! raw `FunctionPass`/`ModulePass` trait directly; both share the exact same body
//! (through a common helper) so any divergence is the macro expansion's fault. A
//! test runs each pass through the real single-pass drivers and asserts (a) the
//! same IR mutation, (b) the same `Verified`/`Unverified` output typestate
//! binding (the explicit annotation is the compile-time half of the lock), and
//! (c) that `name`/`access`/`requires`/`required` mapped onto the right trait
//! members (`NAME`/`Access`/`Requires`/`REQUIRED`).
//!
//! D11 provenance: llvmkit-specific macro-authoring lock — the
//! `#[function_pass]`/`#[module_pass]` sugar expands to the raw trait impl; no
//! upstream analog (LLVM pass registration is `PassInfoMixin` + manual plugin
//! wiring). The drivers these exercise port `unittests/IR/PassManagerTest.cpp`.

use std::cell::Cell;
use std::rc::Rc;

use llvmkit_ir::{
    Analyses, DominatorTreeAnalysis, FnCx, FnPatch, FnReport, FunctionPass, FunctionView,
    IRBuilder, InstructionView, IrError, IrResult, Linkage, ModCx, ModReport, Module, ModuleBrand,
    ModulePass, NoFolder, PatchBody, RewriteModule, Type, Unverified, Verified, function_pass,
    module_pass, run_function_pass, run_module_pass,
};

// ==========================================================================
// Fixtures
// ==========================================================================

/// `i32 @f()` with one unused `add` named `dead` before the terminator. Built
/// with [`NoFolder`] so the constant add survives to be a real trivially-dead
/// instruction (mirrors `tests/pipeline_basic.rs`).
fn build_dead_add<'ctx, B: ModuleBrand + 'ctx>(
    m: &Module<'ctx, B, Unverified>,
) -> Result<FunctionView<'ctx, B>, IrError> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, Vec::<Type<'ctx, B>>::new(), false);
    let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
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

/// `i32 @f()` whose entry just returns a constant — no dead instruction.
fn build_ret_i32<'ctx, B: ModuleBrand + 'ctx>(
    m: &Module<'ctx, B, Unverified>,
) -> Result<FunctionView<'ctx, B>, IrError> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, Vec::<Type<'ctx, B>>::new(), false);
    let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block(m, "entry");
    let b = IRBuilder::new_for::<i32>(m).position_at_end(entry);
    b.build_ret(i32_ty.const_int(1_u32))?;
    Ok(f.into())
}

/// The single mutating body shared by the macro pass and its hand-written twin:
/// erase every use-less non-terminator in the function. Both passes delegate
/// here, so the *only* difference between them is the impl header the macro
/// hides.
fn erase_dead_instructions<'ctx, B: ModuleBrand + 'ctx>(
    patch: &mut FnPatch<'_, '_, 'ctx, B, ()>,
) -> IrResult<()> {
    let mut dead: Vec<InstructionView<'ctx, B>> = Vec::new();
    for block in patch.function_mut().basic_blocks() {
        for view in block.instructions() {
            if !view.as_value().has_uses() && !view.is_terminator() {
                dead.push(view);
            }
        }
    }
    for view in &dead {
        patch.erase(
            &view
                .as_non_terminator()
                .expect("filtered to non-terminators"),
        );
    }
    Ok(())
}

/// Meta reader: yields a function pass's `NAME`/`REQUIRED` consts. The `witness`
/// module fixes `'ctx`/`B` and `_pass` fixes `P` by inference, so no un-nameable
/// brand has to be spelled.
fn fn_pass_meta<'ctx, B, P>(_witness: &Module<'ctx, B, Verified>, _pass: &P) -> (&'static str, bool)
where
    B: ModuleBrand + 'ctx,
    P: FunctionPass<'ctx, B>,
{
    (P::NAME, P::REQUIRED)
}

/// Meta reader for a module pass — the module-level mirror of [`fn_pass_meta`].
fn mod_pass_meta<'ctx, B, P>(
    _witness: &Module<'ctx, B, Verified>,
    _pass: &P,
) -> (&'static str, bool)
where
    B: ModuleBrand + 'ctx,
    P: ModulePass<'ctx, B>,
{
    (P::NAME, P::REQUIRED)
}

// ==========================================================================
// Function pass: `#[function_pass]` PatchBody eraser vs hand-written twin
// ==========================================================================

/// Macro-authored `PatchBody` eraser. The `cx: FnCx<Self>` and `-> IrResult<..>`
/// are readability sentinels the macro overrides with the canonical signature.
struct MacroEraser;

#[function_pass(name = "macro-dce", access = PatchBody)]
impl MacroEraser {
    fn run(&mut self, cx: FnCx<Self>) -> IrResult<FnReport> {
        let mut patch = cx.mutate();
        erase_dead_instructions(&mut patch)?;
        Ok(patch.done())
    }
}

/// Hand-written twin implementing the raw trait — identical body.
struct HandEraser;

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for HandEraser {
    type Access = PatchBody;
    type Requires = ();
    const NAME: &'static str = "macro-dce";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, PatchBody, ()>) -> IrResult<FnReport> {
        let mut patch = cx.mutate();
        erase_dead_instructions(&mut patch)?;
        Ok(patch.done())
    }
}

#[test]
fn macro_function_pass_matches_handwritten() -> Result<(), IrError> {
    // Macro-authored pass: the `PatchBody` rung downgrades the module, so the
    // explicit `Unverified` binding is the compile-time half of the lock.
    let macro_ir: String = Module::with_new("macro-fn", |m| {
        let f = build_dead_add(&m)?;
        let verified = m.verify()?;
        let mut analyses = Analyses::new();

        let (name, required) = fn_pass_meta(&verified, &MacroEraser);
        assert_eq!(name, "macro-dce", "`name` must map onto NAME");
        assert!(
            !required,
            "no `required` flag ⇒ REQUIRED stays the false default"
        );

        let out: Module<'_, _, Unverified> =
            run_function_pass(MacroEraser, verified, f, &mut analyses)?;
        Ok(format!("{}", out.verify()?))
    })?;

    // Hand-written twin over an identical module.
    let hand_ir: String = Module::with_new("hand-fn", |m| {
        let f = build_dead_add(&m)?;
        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let out: Module<'_, _, Unverified> =
            run_function_pass(HandEraser, verified, f, &mut analyses)?;
        Ok(format!("{}", out.verify()?))
    })?;

    // Same erase, and (module name aside) byte-identical IR.
    assert!(
        !macro_ir.contains("%dead"),
        "the macro pass erased the dead add"
    );
    assert_eq!(
        macro_ir.replace("macro-fn", "MOD"),
        hand_ir.replace("hand-fn", "MOD"),
        "macro expansion must behave identically to the hand-written impl",
    );
    Ok(())
}

// ==========================================================================
// Function pass: `requires = [..]` prefetch + infallible accessor
// ==========================================================================

/// Macro-authored `Inspect` pass declaring a required analysis. That
/// `cx.analysis::<DominatorTreeAnalysis, _>()` compiles at all is the proof the
/// macro emitted `type Requires = (DominatorTreeAnalysis,)` — an undeclared
/// analysis has no `AnalysisSelector` impl.
struct MacroAnalysisReader {
    reachable: Rc<Cell<bool>>,
}

#[function_pass(name = "macro-dt-reader", access = Inspect, requires = [DominatorTreeAnalysis])]
impl MacroAnalysisReader {
    fn run(&mut self, cx: FnCx<Self>) -> IrResult<FnReport> {
        let dt = cx.analysis::<DominatorTreeAnalysis, _>();
        let entry = cx
            .function()
            .entry_block()
            .expect("definition has an entry block");
        self.reachable.set(dt.is_reachable_from_entry(entry));
        Ok(cx.done())
    }
}

#[test]
fn macro_function_pass_with_requires_reads_analysis_and_stays_verified() -> Result<(), IrError> {
    Module::with_new("macro-requires", |m| {
        let f = build_ret_i32(&m)?;
        let verified = m.verify()?;
        let mut analyses = Analyses::new();

        let reachable = Rc::new(Cell::new(false));
        let pass = MacroAnalysisReader {
            reachable: reachable.clone(),
        };

        let (name, required) = fn_pass_meta(&verified, &pass);
        assert_eq!(name, "macro-dt-reader");
        assert!(!required);

        // The `Inspect` rung keeps the module verified: the explicit `Verified`
        // annotation is the compile-time half of the lock.
        let out: Module<'_, _, Verified> = run_function_pass(pass, verified, f, &mut analyses)?;

        assert!(
            reachable.get(),
            "the required DominatorTreeAnalysis was prefetched and read through cx.analysis()"
        );
        assert!(
            format!("{out}").contains("ret i32 1"),
            "a read-only pass leaves the IR untouched"
        );
        Ok(())
    })
}

// ==========================================================================
// Module pass: `#[module_pass]` RewriteModule + `required` vs hand-written twin
// ==========================================================================

/// Macro-authored `RewriteModule` module pass with the bare `required` flag.
struct MacroAddGlobal;

#[module_pass(name = "macro-add-global", access = RewriteModule, required)]
impl MacroAddGlobal {
    fn run(&mut self, cx: ModCx<Self>) -> IrResult<ModReport> {
        let rewrite = cx.mutate();
        let i32_ty = rewrite.module_mut().i32_type();
        rewrite.module_mut().add_global("g", i32_ty.const_zero())?;
        Ok(rewrite.done())
    }
}

/// Hand-written twin — identical body, raw trait impl.
struct HandAddGlobal;

impl<'ctx, B: ModuleBrand + 'ctx> ModulePass<'ctx, B> for HandAddGlobal {
    type Access = RewriteModule;
    type Requires = ();
    const NAME: &'static str = "macro-add-global";
    const REQUIRED: bool = true;

    fn run(&mut self, cx: ModCx<'_, '_, '_, 'ctx, B, RewriteModule, ()>) -> IrResult<ModReport> {
        let rewrite = cx.mutate();
        let i32_ty = rewrite.module_mut().i32_type();
        rewrite.module_mut().add_global("g", i32_ty.const_zero())?;
        Ok(rewrite.done())
    }
}

#[test]
fn macro_module_pass_matches_handwritten() -> Result<(), IrError> {
    let macro_ir: String = Module::with_new("macro-mod", |m| {
        let _f = build_ret_i32(&m)?;
        let verified = m.verify()?;
        assert_eq!(verified.iter_globals().len(), 0);
        let mut analyses = Analyses::new();

        let (name, required) = mod_pass_meta(&verified, &MacroAddGlobal);
        assert_eq!(name, "macro-add-global");
        assert!(
            required,
            "the bare `required` flag must map onto REQUIRED = true"
        );

        // `RewriteModule` downgrades: the explicit `Unverified` binding is the
        // compile-time half of the lock.
        let out: Module<'_, _, Unverified> =
            run_module_pass(MacroAddGlobal, verified, &mut analyses)?;
        assert_eq!(
            out.iter_globals().len(),
            1,
            "the macro pass added the global"
        );
        Ok(format!("{}", out.verify()?))
    })?;

    let hand_ir: String = Module::with_new("hand-mod", |m| {
        let _f = build_ret_i32(&m)?;
        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let out: Module<'_, _, Unverified> =
            run_module_pass(HandAddGlobal, verified, &mut analyses)?;
        Ok(format!("{}", out.verify()?))
    })?;

    assert_eq!(
        macro_ir.replace("macro-mod", "MOD"),
        hand_ir.replace("hand-mod", "MOD"),
        "macro expansion must behave identically to the hand-written impl",
    );
    Ok(())
}
