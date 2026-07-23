//! Executed test coverage for the single-pass module driver
//! (`run_module_pass`) and the read-only (`Inspect`) paths at both the
//! module and function level.
//!
//! `crates/llvmkit-ir/src/pass_manager.rs` ships `run_function_pass` and
//! `run_module_pass`, but only `run_function_pass` had executed coverage
//! before this file — and only at the mutating `PatchBody` rung, via
//! `DcePass`/`InstSimplifyPass` in `scalar_cleanup_passes.rs`. The module
//! driver and both `Inspect` paths were compile-checked only: the Task 3
//! cutover deleted the old runtime module-pass verification tests
//! (`type_safety_brand.rs::read_only_module_pass_returns_verified` /
//! `transform_module_pass_returns_unverified`) without a runtime
//! replacement. These tests drive real `ModulePass`/`FunctionPass` impls
//! through the real drivers and check both the verified/unverified typestate
//! binding and that the pass genuinely ran (a shared `Rc<Cell<bool>>` flag
//! flips, plus an in-`run` assertion).
//!
//! Context-construction idiom ported from
//! `pass_context.rs`'s `#[cfg(test)] mod tests` (building a verified module,
//! an analysis manager, and a pass-author context / mutator).

use std::cell::Cell;
use std::rc::Rc;

use llvmkit_ir::{
    Analyses, Dyn, FnCx, FnReport, FunctionPass, IRBuilder, Inspect, IrError, IrResult, Linkage,
    ModCx, ModReport, Module, ModuleBrand, ModulePass, RewriteModule, Unverified, Verified,
    run_function_pass, run_module_pass,
};

/// Read-only module pass: counts the module's functions and flips a shared
/// flag, so a passing test proves both a correct read *and* that `run` was
/// actually invoked (not just type-checked).
struct CountFunctionsPass {
    ran: Rc<Cell<bool>>,
}

impl<'ctx, B: ModuleBrand + 'ctx> ModulePass<'ctx, B> for CountFunctionsPass {
    type Access = Inspect;
    type Requires = ();
    const NAME: &'static str = "count-functions-probe";

    fn run(&mut self, cx: ModCx<'_, '_, '_, 'ctx, B, Inspect, ()>) -> IrResult<ModReport> {
        assert_eq!(
            cx.functions().count(),
            1,
            "the Inspect ModulePass must see the one defined function"
        );
        self.ran.set(true);
        Ok(cx.done())
    }
}

/// llvmkit-specific single-pass-driver verification lock (no upstream
/// analog: LLVM pass managers have no compile-time verified/unverified
/// typestate). An `Inspect` `ModulePass` run through [`run_module_pass`]
/// must bind as `Module<'_, _, Verified>` (read-only stays verified) *and*
/// must have genuinely executed.
#[test]
fn inspect_module_pass_stays_verified_and_runs() -> Result<(), IrError> {
    Module::with_new("inspect-module-pass", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        b.build_ret(i32_ty.const_int(1_u32))?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();

        let ran = Rc::new(Cell::new(false));
        let pass = CountFunctionsPass { ran: ran.clone() };

        // The explicit `Verified` annotation is the compile-time half of the
        // assertion: a wrong driver verdict here fails to compile.
        let out: Module<'_, _, Verified> = run_module_pass(pass, verified, &mut analyses)?;

        assert!(ran.get(), "Inspect ModulePass::run must actually execute");
        assert_eq!(out.as_view().functions().count(), 1);
        Ok(())
    })
}

/// Mutating module pass: adds a global straight through the `RewriteModule`
/// mutator's raw module token, and flips a shared flag.
struct AddGlobalPass {
    ran: Rc<Cell<bool>>,
}

impl<'ctx, B: ModuleBrand + 'ctx> ModulePass<'ctx, B> for AddGlobalPass {
    type Access = RewriteModule;
    type Requires = ();
    const NAME: &'static str = "add-global-probe";

    fn run(&mut self, cx: ModCx<'_, '_, '_, 'ctx, B, RewriteModule, ()>) -> IrResult<ModReport> {
        let rewrite = cx.mutate();
        let i32_ty = rewrite.module_mut().i32_type();
        rewrite.module_mut().add_global("g", i32_ty.const_zero())?;
        self.ran.set(true);
        Ok(rewrite.done())
    }
}

/// llvmkit-specific single-pass-driver verification lock (no upstream
/// analog: LLVM pass managers have no compile-time verified/unverified
/// typestate). A `RewriteModule` `ModulePass` run through [`run_module_pass`]
/// must bind as `Module<'_, _, Unverified>` (D8 downgrade) *and* its
/// mutation must be observable on the returned module.
#[test]
fn rewrite_module_pass_downgrades_and_mutates() -> Result<(), IrError> {
    Module::with_new("rewrite-module-pass", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        b.build_ret(i32_ty.const_int(0_u32))?;

        let verified = m.verify()?;
        assert_eq!(verified.globals().len(), 0);

        let mut analyses = Analyses::new();

        let ran = Rc::new(Cell::new(false));
        let pass = AddGlobalPass { ran: ran.clone() };

        // The explicit `Unverified` annotation is the compile-time half of
        // the assertion: a wrong driver verdict here fails to compile.
        let out: Module<'_, _, Unverified> = run_module_pass(pass, verified, &mut analyses)?;

        assert!(
            ran.get(),
            "RewriteModule ModulePass::run must actually execute"
        );
        // The real mutation landed on the returned module, not just inside
        // the pass's own scope.
        assert_eq!(out.globals().len(), 1);
        // The mutation is a real, well-formed IR edit.
        out.verify()?;
        Ok(())
    })
}

/// Read-only function pass: reads the function's block count and flips a
/// shared flag.
struct InspectFnPass {
    ran: Rc<Cell<bool>>,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for InspectFnPass {
    type Access = Inspect;
    type Requires = ();
    const NAME: &'static str = "inspect-fn-probe";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, Inspect, ()>) -> IrResult<FnReport> {
        assert_eq!(
            cx.function().basic_blocks().count(),
            1,
            "the Inspect FunctionPass must see the one basic block"
        );
        self.ran.set(true);
        Ok(cx.done())
    }
}

/// llvmkit-specific single-pass-driver verification lock (no upstream
/// analog: LLVM pass managers have no compile-time verified/unverified
/// typestate). An `Inspect` `FunctionPass` run through [`run_function_pass`]
/// must bind as `Module<'_, _, Verified>` *and* must have genuinely
/// executed. (`run_function_pass` already has executed coverage at the
/// mutating `PatchBody` rung via `DcePass`/`InstSimplifyPass` in
/// `scalar_cleanup_passes.rs`; this closes the `Inspect`-rung gap.)
#[test]
fn inspect_function_pass_stays_verified_and_runs() -> Result<(), IrError> {
    Module::with_new("inspect-function-pass", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type_no_params(i32_ty, false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        b.build_ret(i32_ty.const_int(1_u32))?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();

        let ran = Rc::new(Cell::new(false));
        let pass = InspectFnPass { ran: ran.clone() };

        // The explicit `Verified` annotation is the compile-time half of the
        // assertion: a wrong driver verdict here fails to compile.
        let out: Module<'_, _, Verified> = run_function_pass(pass, verified, f, &mut analyses)?;

        assert!(ran.get(), "Inspect FunctionPass::run must actually execute");
        // A read-only pass leaves the IR untouched.
        assert!(format!("{out}").contains("ret i32 1"));
        Ok(())
    })
}
