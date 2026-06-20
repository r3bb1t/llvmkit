//! Branded module identity and verified-pass-state coverage.
//!
//! Every test cites its upstream source per Doctrine D11.

use std::collections::HashMap;

use llvmkit_ir::{
    FunctionAnalysisManager, FunctionPass, FunctionPassContext, FunctionPassManager, IRBuilder,
    IntValue, IrResult, Linkage, Module, ModuleAnalysisManager, ModulePass, ModulePassContext,
    ModulePassManager, ModuleToFunctionPassAdaptor, MutatesIr, PreservedAnalyses,
    PreservesVerification, ReadOnlyModulePass, ReadOnlyModulePassContext, Type, Unverified, Value,
    Verified,
};

fn exercise_tables<'ctx>(module: Module<'ctx>) -> IrResult<()> {
    let i64_ty = module.i64_type();
    let fn_ty = module.fn_type(i64_ty.as_type(), [i64_ty.as_type()], false);
    let function = module.add_function::<i64>("f", fn_ty, Linkage::External)?;
    let entry = function.append_basic_block(&module, "entry");
    let parameter: IntValue<'ctx, i64> = function.param(0)?.try_into()?;

    let mut values = HashMap::<&str, Value<'ctx>>::new();
    values.insert("parameter", parameter.as_value());
    let mut integers = HashMap::<&str, IntValue<'ctx, i64>>::new();
    integers.insert("parameter", parameter);

    let lhs = *integers.get("parameter").expect("int value");
    let rhs: IntValue<'ctx, i64> = (*values.get("parameter").expect("value")).try_into()?;
    let builder = IRBuilder::new_for::<i64>(&module).position_at_end(entry);
    let sum = builder.build_int_add(lhs, rhs, "sum")?;
    builder.build_ret(sum)?;

    let text = format!("{module}");
    assert!(text.contains("add i64"));
    Ok(())
}

struct ReadOnlyShapePass;

impl<'ctx> ReadOnlyModulePass<'ctx> for ReadOnlyShapePass {
    fn run(&mut self, cx: &mut ReadOnlyModulePassContext<'_, 'ctx>) -> IrResult<PreservedAnalyses> {
        assert_eq!(cx.module().name(), "read-only-pass-state");
        assert_eq!(cx.functions().len(), 0);
        Ok(PreservedAnalyses::all())
    }
}

struct AppendAsmPass;

impl<'ctx> ModulePass<'ctx> for AppendAsmPass {
    fn run(&mut self, cx: &mut ModulePassContext<'_, 'ctx>) -> IrResult<PreservedAnalyses> {
        cx.module_mut().append_module_asm("side effect");
        Ok(PreservedAnalyses::none())
    }
}

struct FunctionAttrPass;

impl<'ctx> FunctionPass<'ctx> for FunctionAttrPass {
    fn run(&mut self, cx: &mut FunctionPassContext<'_, 'ctx>) -> IrResult<PreservedAnalyses> {
        let module = cx.module_mut();
        cx.function_mut()
            .as_function()
            .set_linkage(module, Linkage::Internal);
        Ok(PreservedAnalyses::all())
    }
}

/// `llvmkit-specific D7`: user-owned value tables retain the module brand,
/// so values can be stored, retrieved, and reused without weakening to runtime
/// module-id checks.
#[test]
fn user_owned_value_tables_remain_usable() -> IrResult<()> {
    Module::with_new::<_, _, _>("brand-tables", exercise_tables)
}

/// `llvmkit-specific D8`: a read-only module pipeline preserves
/// `Module<'ctx, B, Verified>` in its return type.
#[test]
fn read_only_module_pass_returns_verified() -> IrResult<()> {
    Module::with_new::<_, _, _>("read-only-pass-state", |module| {
        let verified = module.verify()?;
        let mut mpm = ModulePassManager::<_, PreservesVerification>::new_read_only();
        mpm.add_pass(ReadOnlyShapePass);
        let mut mam = ModuleAnalysisManager::new();
        let mut fam = FunctionAnalysisManager::new();
        let verified: Module<'_, _, Verified> = mpm.run(verified, &mut mam, &mut fam)?;
        assert_eq!(verified.name(), "read-only-pass-state");
        Ok(())
    })
}

/// `llvmkit-specific D8`: transform module pipelines return
/// `Module<'ctx, B, Unverified>`, requiring an explicit verifier pass before
/// the module can enter another verified-only pass pipeline.
#[test]
fn transform_module_pass_returns_unverified() -> IrResult<()> {
    Module::with_new::<_, _, _>("pass-state", |module| {
        let verified = module.verify()?;
        let mut mpm = ModulePassManager::<_, MutatesIr>::new_transform();
        mpm.add_pass(AppendAsmPass);
        let mut mam = ModuleAnalysisManager::new();
        let mut fam = FunctionAnalysisManager::new();
        let unverified: Module<'_, _, Unverified> = mpm.run(verified, &mut mam, &mut fam)?;
        let reverified = unverified.verify()?;
        assert!(format!("{reverified}").contains("module asm \"side effect\""));
        Ok(())
    })
}

/// `llvmkit-specific D8`: function-body mutation through a transform
/// module-to-function adaptor returns an unverified enclosing module.
#[test]
fn transform_function_adaptor_returns_unverified() -> IrResult<()> {
    Module::with_new::<_, _, _>("function-pass-state", |module| {
        let void_ty = module.void_type();
        let fn_ty = module.fn_type(void_ty.as_type(), Vec::<Type>::new(), false);
        let function = module.add_function::<()>("f", fn_ty, Linkage::External)?;
        let entry = function.append_basic_block(&module, "entry");
        IRBuilder::new_for::<()>(&module)
            .position_at_end(entry)
            .build_ret_void();

        let verified = module.verify()?;
        let mut fpm = FunctionPassManager::<_, MutatesIr>::new_transform();
        fpm.add_pass(FunctionAttrPass);
        let mut mpm = ModulePassManager::<_, MutatesIr>::new_transform();
        mpm.add_pass(ModuleToFunctionPassAdaptor::new(fpm));
        let mut mam = ModuleAnalysisManager::new();
        let mut fam = FunctionAnalysisManager::new();
        let unverified: Module<'_, _, Unverified> = mpm.run(verified, &mut mam, &mut fam)?;
        let reverified = unverified.verify()?;
        assert!(format!("{reverified}").contains("define internal void @f()"));
        Ok(())
    })
}
