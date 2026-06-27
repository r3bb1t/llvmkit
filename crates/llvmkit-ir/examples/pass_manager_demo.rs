//! Demonstrates the minimal new-pass-manager-inspired surface that
//! `llvmkit-ir` ships today:
//! - build IR with the typed `IRBuilder`
//! - run the built-in `DominatorTreeAnalysis`
//! - run a custom module pass
//! - run a custom function pass through `ModuleToFunctionPassAdaptor`
//!
//! Run with:
//!
//! ```text
//! cargo run -p llvmkit-ir --example pass_manager_demo
//! ```

use std::cell::RefCell;
use std::rc::Rc;

use llvmkit_ir::{
    DominatorTreeAnalysis, FunctionAnalysisManager, FunctionPassManager, IRBuilder, IntPredicate,
    IrError, Linkage, Module, ModuleAnalysisManager, ModulePassManager,
    ModuleToFunctionPassAdaptor, PreservedAnalyses, PreservesVerification, ReadOnlyFunctionPass,
    ReadOnlyFunctionPassContext, ReadOnlyModulePass, ReadOnlyModulePassContext,
};

struct ReportModulePass {
    out: Rc<RefCell<Vec<String>>>,
}

impl<'ctx> ReadOnlyModulePass<'ctx> for ReportModulePass {
    fn run(
        &mut self,
        cx: &mut ReadOnlyModulePassContext<'_, 'ctx>,
    ) -> Result<PreservedAnalyses, IrError> {
        self.out.borrow_mut().push(format!(
            "module_pass functions={}",
            cx.module().iter_functions().len()
        ));
        Ok(PreservedAnalyses::all())
    }
}

struct ReportFunctionPass {
    out: Rc<RefCell<Vec<String>>>,
}

impl<'ctx> ReadOnlyFunctionPass<'ctx> for ReportFunctionPass {
    fn run(
        &mut self,
        cx: &mut ReadOnlyFunctionPassContext<'_, 'ctx>,
    ) -> Result<PreservedAnalyses, IrError> {
        let function = cx.function();
        let dt = cx.analysis::<DominatorTreeAnalysis>()?;
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
        Ok(PreservedAnalyses::all())
    }
}

pub fn build(m: &Module<'_>) -> Result<(), IrError> {
    let f = m.add_typed_function::<i32, (bool, i32, i32), _>("select_or_add", Linkage::External)?;
    let entry = f.append_basic_block(m, "entry");
    let then_bb = f.append_basic_block(m, "then");
    let else_bb = f.append_basic_block(m, "else");
    let merge = f.append_basic_block(m, "merge");
    let then_label = then_bb.label();
    let else_label = else_bb.label();
    let merge_label = merge.label();

    let (cond, x, y) = f.params();

    IRBuilder::new_for::<i32>(m)
        .position_at_end(entry)
        .build_cond_br(cond, then_label, else_label)?;

    let bt = IRBuilder::new_for::<i32>(m).position_at_end(then_bb);
    let add_xy = bt.build_int_add(x, y, "add_xy")?;
    bt.build_br(merge_label)?;

    let be = IRBuilder::new_for::<i32>(m).position_at_end(else_bb);
    let sub_xy = be.build_int_sub(x, y, "sub_xy")?;
    be.build_br(merge_label)?;

    let bm = IRBuilder::new_for::<i32>(m).position_at_end(merge);
    let phi = bm
        .build_int_phi::<i32, _>("result")?
        .add_incoming(add_xy, then_label)?
        .add_incoming(sub_xy, else_label)?;
    let is_zero = bm.build_int_cmp(IntPredicate::Eq, phi.as_int_value(), 0_i32, "is_zero")?;
    let selected = bm.build_select(is_zero, x, phi.as_int_value(), "selected")?;
    bm.build_ret(selected)?;
    Ok(())
}

pub fn run_demo(m: Module<'_>) -> Result<(String, String), IrError> {
    let function = m
        .function_by_name("select_or_add")
        .expect("demo function is present");
    let entry = function
        .entry_block()
        .expect("demo function has an entry block");
    let merge = function
        .basic_blocks()
        .find(|bb| bb.name().as_deref() == Some("merge"))
        .expect("demo function has a merge block");

    let mut fam = FunctionAnalysisManager::new();
    fam.register_pass(DominatorTreeAnalysis);
    let dt = fam.get_result::<DominatorTreeAnalysis, _>(function)?;

    let lines = Rc::new(RefCell::new(vec![format!(
        "analysis entry_dominates_merge={}",
        dt.dominates_block(entry, merge)
    )]));

    let mut fpm = FunctionPassManager::<_, PreservesVerification>::new_read_only();
    fpm.add_pass(ReportFunctionPass { out: lines.clone() });

    let mut mpm = ModulePassManager::<_, PreservesVerification>::new_read_only();
    mpm.add_pass(ReportModulePass { out: lines.clone() });
    mpm.add_pass(ModuleToFunctionPassAdaptor::new(fpm));

    let mut mam = ModuleAnalysisManager::new();
    let module = mpm.run(m.verify()?, &mut mam, &mut fam)?;
    let report = lines.borrow().join("\n");
    let module_text = format!("{module}");
    Ok((report, module_text))
}

pub fn main() {
    let (report, module_text) = match Module::with_new("pass_manager_demo", |m| {
        build(&m)?;
        run_demo(m)
    }) {
        Ok(output) => output,
        Err(err) => {
            eprintln!("error: {err:?}");
            std::process::exit(1);
        }
    };

    println!("{report}");
    print!("{module_text}");
}
