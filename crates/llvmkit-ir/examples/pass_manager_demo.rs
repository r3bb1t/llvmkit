//! Demonstrates the capability-graded single-pass driver `llvmkit-ir` ships
//! today:
//! - build IR with the typed `IRBuilder`
//! - run the mutating `InstSimplifyPass`/`DcePass` through `run_function_pass`
//!   (each declares the `PatchBody` rung, so the driver downgrades the module to
//!   `Module<Unverified>` and the re-verify below is required by the type system)
//! - run the built-in `DominatorTreeAnalysis`
//! - run a read-only (`Inspect`) module pass through `run_module_pass`
//! - run a read-only (`Inspect`) function pass through `run_function_pass`
//!
//! Run with:
//!
//! ```text
//! cargo run -p llvmkit-ir --example pass_manager_demo
//! ```

use std::cell::RefCell;
use std::rc::Rc;

use llvmkit_ir::{
    Analyses, DcePass, DominatorTreeAnalysis, FnCx, FnReport, FunctionPass, IRBuilder, Inspect,
    InstSimplifyPass, IntPredicate, IntValue, IrError, Linkage, ModCx, ModReport, Module,
    ModuleBrand, ModulePass, module_new, run_function_pass, run_module_pass,
};

/// Read-only module pass: reports how many functions the module holds. Declares
/// the `Inspect` rung, so it can only `done()` (never mutate) and the driver
/// keeps the module `Verified`.
struct ReportModulePass {
    out: Rc<RefCell<Vec<String>>>,
}

impl<B: ModuleBrand> ModulePass<B> for ReportModulePass {
    type Access = Inspect;
    type Requires = ();
    const NAME: &'static str = "report-module";

    fn run<'m, 'ctx>(
        &mut self,
        cx: ModCx<'m, '_, '_, 'ctx, B, Inspect, ()>,
    ) -> Result<ModReport, IrError>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        self.out.borrow_mut().push(format!(
            "module_pass functions={}",
            cx.module().functions().len()
        ));
        Ok(cx.done())
    }
}

/// Read-only function pass: reports whether the entry block dominates `merge`,
/// reading the prefetched `DominatorTreeAnalysis` through the infallible
/// accessor. Declares the `Inspect` rung.
struct ReportFunctionPass {
    out: Rc<RefCell<Vec<String>>>,
}

impl<B: ModuleBrand> FunctionPass<B> for ReportFunctionPass {
    type Access = Inspect;
    type Requires = (DominatorTreeAnalysis,);
    const NAME: &'static str = "report-function";

    fn run<'m, 'ctx>(
        &mut self,
        cx: FnCx<'m, '_, 'ctx, B, Inspect, (DominatorTreeAnalysis,)>,
    ) -> Result<FnReport, IrError>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let function = cx.function();
        let dt = cx.analysis::<DominatorTreeAnalysis, _>();
        let entry = function
            .entry_block()
            .expect("demo function has an entry block");
        let merge = function
            .basic_blocks()
            .find(|bb| bb.name().as_deref() == Some("merge"))
            .expect("demo function has a merge block");
        self.out.borrow_mut().push(format!(
            "function_pass {} entry_dominates_merge={}",
            function.name(),
            dt.dominates_block(entry, merge)
        ));
        Ok(cx.done())
    }
}

pub fn build<B: ModuleBrand>(m: &Module<B>) -> Result<(), IrError> {
    let i32_ty = m.i32_type();
    let f = m.add_typed_function::<i32, (bool, i32, i32), _>("select_or_add", Linkage::External)?;
    let entry = m.view(f).append_basic_block(m, "entry");
    let then_bb = m.view(f).append_basic_block(m, "then");
    let else_bb = m.view(f).append_basic_block(m, "else");
    // `merge`'s single `i32` parameter is the diamond's head-phi: the `then`
    // and `else` arms carry their values in as block arguments below.
    let bwp = IRBuilder::new_for::<i32>(m);
    let (merge, params) =
        bwp.append_block_with_params(m.view(f).as_function(), &[i32_ty.as_type()], "merge")?;
    let then_label = then_bb.id();
    let else_label = else_bb.id();
    let merge_label = merge.id();

    let (cond, x, y) = m.view(f).params();

    IRBuilder::at_end(entry).build_cond_br(cond, then_label, else_label)?;

    let bt = IRBuilder::at_end(then_bb);
    let add_xy = bt.build_int_add(x, y, "add_xy")?;
    bt.build_br_with_args(merge_label, &[m.view(add_xy).into_erased()])?;

    let be = IRBuilder::at_end(else_bb);
    let sub_xy = be.build_int_sub(x, y, "sub_xy")?;
    be.build_br_with_args(merge_label, &[m.view(sub_xy).into_erased()])?;

    let bm = IRBuilder::at_end(merge);
    // `params[0]` is `merge`'s head-phi, seeded with `[ %add_xy, %then ]` and
    // `[ %sub_xy, %else ]` by the two block-argument branches above.
    let result: IntValue<'_, i32, _> = params[0].try_into()?;
    let is_zero = bm.build_int_cmp(IntPredicate::Eq, result, 0_i32, "is_zero")?;
    let selected = bm.build_select(is_zero, x, result, "selected")?;
    bm.build_ret(selected)?;
    Ok(())
}

pub fn run_demo<B: ModuleBrand>(m: Module<B>) -> Result<(String, String, String), IrError> {
    // Everything the drivers below are handed is an **id**: each `run_*_pass`
    // consumes the module token, so a handle minted here would be a borrow of a
    // token that is about to move. Ids survive the move; views are re-minted on
    // the far side against whichever module the run produced.
    let function = m
        .function_by_name_dyn("select_or_add")
        .expect("demo function is present")
        .id();
    let entry = m
        .view(function)
        .entry_block()
        .expect("demo function has an entry block")
        .id();
    let merge = m
        .view(function)
        .basic_blocks()
        .find(|bb| bb.name().as_deref() == Some("merge"))
        .expect("demo function has a merge block")
        .id();

    // Mutating cleanup: `InstSimplifyPass` and `DcePass` each declare the
    // `PatchBody` rung, so `run_function_pass` downgrades the module to
    // `Module<Unverified>` and the re-verify between them is enforced by the
    // type system (D8), not convention.
    let mut analyses = Analyses::new();
    let simplified = run_function_pass(InstSimplifyPass, m.verify()?, function, &mut analyses)?;
    let cleaned = run_function_pass(DcePass, simplified.verify()?, function, &mut analyses)?;
    let module = cleaned.verify()?;
    let cleaned_module_text = format!("{module}");

    // Read-only reporting/analysis flow. `DominatorTreeAnalysis` is registered
    // here for the direct query; `ReportFunctionPass` re-declares it as a
    // `Requires` so the driver prefetches it for the infallible accessor.
    analyses.register_function_analysis(DominatorTreeAnalysis);
    let dt = analyses
        .function_manager_mut()
        .get_result::<DominatorTreeAnalysis, _>(module.view(function))?;

    let lines = Rc::new(RefCell::new(vec![format!(
        "analysis entry_dominates_merge={}",
        dt.dominates_block(entry, merge)
    )]));

    // Both passes declare the `Inspect` rung, so the driver keeps the module
    // `Verified` on the way out.
    let module = run_module_pass(
        ReportModulePass { out: lines.clone() },
        module,
        &mut analyses,
    )?;
    let module = run_function_pass(
        ReportFunctionPass { out: lines.clone() },
        module,
        function,
        &mut analyses,
    )?;

    let report = lines.borrow().join("\n");
    let module_text = format!("{module}");
    Ok((cleaned_module_text, report, module_text))
}

/// The module is minted here rather than in `main` so `?` has a `Result` to
/// return to: `module_new!` is fallible (its brand is a registry key) and
/// `main` reports errors by hand.
fn emit() -> Result<(String, String, String), IrError> {
    let m = module_new!("pass_manager_demo")?;
    build(&m)?;
    run_demo(m)
}

pub fn main() {
    let (cleaned_module_text, report, module_text) = match emit() {
        Ok(output) => output,
        Err(err) => {
            eprintln!("error: {err:?}");
            std::process::exit(1);
        }
    };

    println!("after scalar cleanup passes:");
    print!("{cleaned_module_text}");
    println!("{report}");
    print!("{module_text}");
}
