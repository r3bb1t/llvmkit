//! End-to-end demo of the `#[function_pass]` / `#[module_pass]` authoring sugar.
//!
//! Each pass is written as a plain inherent `impl Pass { fn run(..) }` block; the
//! attribute macro expands it to exactly the raw `FunctionPass`/`ModulePass`
//! trait impl — the `impl<'ctx, B: ModuleBrand + 'ctx> … for` header, the
//! `type Access`/`type Requires`/`const NAME` block, and the `run` lifetimes are
//! all supplied by the macro. Note that `FnCx<Self>` / `ModCx<Self>` and the
//! `-> IrResult<..>` return are readability sentinels: they are never imported
//! here, because the macro discards them and writes the canonical signature.
//!
//! Run with:
//!
//! ```text
//! cargo run -p llvmkit-ir --example authored_pass
//! ```

use std::cell::Cell;
use std::rc::Rc;

use llvmkit_ir::{
    Analyses, DominatorTreeAnalysis, IRBuilder, IrError, Linkage, Module, Unverified, Verified,
    function_pass, module_pass, run_function_pass, run_module_pass,
};

/// Read-only (`Inspect`) function pass that declares a required analysis and
/// reads it through the infallible accessor — the `requires = [..]` list becomes
/// `type Requires = (DominatorTreeAnalysis,)`, so `cx.analysis()` is infallible.
struct EntryReachable {
    flag: Rc<Cell<bool>>,
}

#[function_pass(name = "entry-reachable", access = Inspect, requires = [DominatorTreeAnalysis])]
impl EntryReachable {
    fn run(&mut self, cx: FnCx<Self>) -> IrResult<FnReport> {
        let dt = cx.analysis::<DominatorTreeAnalysis, _>();
        let entry = cx
            .function()
            .entry_block()
            .expect("definition has an entry block");
        self.flag.set(dt.is_reachable_from_entry(entry));
        Ok(cx.done())
    }
}

/// Mutating (`RewriteModule`) module pass that adds a global through the raw
/// module token exposed by the mutator.
struct AddMarkerGlobal;

#[module_pass(name = "add-marker-global", access = RewriteModule)]
impl AddMarkerGlobal {
    fn run(&mut self, cx: ModCx<Self>) -> IrResult<ModReport> {
        let rewrite = cx.mutate();
        // A Rust literal initializer: no type handle, no `.as_type()`.
        rewrite.module_mut().add_global("marker", 0i32)?;
        Ok(rewrite.done())
    }
}

fn main() -> Result<(), IrError> {
    Module::with_new("authored-pass-demo", |m| {
        // Build `i32 @f()` returning a constant. `add_typed_function` is the
        // typed primary: the turbofish `<i32, ()>` *is* the schema (return
        // `i32`, no parameters) — no separately built `FunctionType`.
        let i32_ty = m.i32_type();
        let f = m.add_typed_function::<i32, (), _>("f", Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::at_end(entry);
        b.build_ret(i32_ty.const_int(1_u32))?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();

        // The `Inspect` function pass keeps the module verified (compile-time
        // half of the guarantee is the explicit `Verified` binding).
        let flag = Rc::new(Cell::new(false));
        let verified: Module<'_, _, Verified> = run_function_pass(
            EntryReachable { flag: flag.clone() },
            verified,
            f.as_function(),
            &mut analyses,
        )?;
        println!("entry-reachable = {}", flag.get());

        // The `RewriteModule` module pass downgrades to `Unverified`.
        let rewritten: Module<'_, _, Unverified> =
            run_module_pass(AddMarkerGlobal, verified, &mut analyses)?;
        println!(
            "globals after add-marker-global = {}",
            rewritten.globals().len()
        );

        let reverified = rewritten.verify()?;
        println!("re-verified module:\n{reverified}");
        Ok(())
    })
}
