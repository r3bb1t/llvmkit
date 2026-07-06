use llvmkit_ir::{
    Brand, DcePass, FunctionAnalysisManager, FunctionPassManager, IRBuilder, InstSimplifyPass,
    IntValue, IrError, Linkage, Module, ModuleAnalysisManager, ModulePassManager,
    ModuleToFunctionPassAdaptor, MutatesIr, NoFolder, PointerValue, Type,
};

/// Port of `llvm/lib/Transforms/Scalar/InstSimplifyPass.cpp::runImpl` and
/// `llvm/include/llvm/Analysis/InstructionSimplify.h`: simplification may
/// replace an instruction with a constant instead of materialising new IR.
#[test]
fn instsimplify_pass_folds_constant_add() -> Result<(), IrError> {
    Module::with_new("instsimplify-pass", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let sum = b.build_int_add::<i32, _, _, _>(
            i32_ty.const_int(40_u32),
            i32_ty.const_int(2_u32),
            "sum",
        )?;
        b.build_ret(sum)?;

        let verified = m.verify()?;
        let mut fpm = FunctionPassManager::<_, MutatesIr>::new_transform();
        fpm.add_pipeline_pass(InstSimplifyPass);
        let mut fam = FunctionAnalysisManager::new();
        let unverified = fpm.run(verified, f, &mut fam)?;
        let reverified = unverified.verify()?;
        let text = format!("{reverified}");

        assert_eq!(
            text,
            concat!(
                "; ModuleID = 'instsimplify-pass'\n",
                "define i32 @f() {\n",
                "entry:\n",
                "  ret i32 42\n",
                "}\n",
            )
        );
        assert!(!text.contains("%sum"), "{text}");
        Ok(())
    })
}

/// Port of `llvm/lib/Transforms/Scalar/DCE.cpp::DCEInstruction` and
/// `llvm/lib/Transforms/Scalar/DCE.cpp::eliminateDeadCode`: recursively dead
/// side-effect-free instructions are erased while stores remain live.
#[test]
fn dce_pass_erases_dead_integer_chain_and_preserves_store() -> Result<(), IrError> {
    Module::with_new("dce-pass", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(m.void_type().as_type(), Vec::<Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let slot = b.build_alloca(i32_ty, "slot")?;
        b.build_store(i32_ty.const_int(7_u32), slot)?;
        let dead0 = b.build_int_add::<i32, _, _, _>(
            i32_ty.const_int(10_u32),
            i32_ty.const_int(20_u32),
            "dead0",
        )?;
        let _dead1 = b.build_int_mul::<i32, _, _, _>(dead0, i32_ty.const_int(3_u32), "dead1")?;
        b.build_ret_void();

        let verified = m.verify()?;
        let mut fpm = FunctionPassManager::<_, MutatesIr>::new_transform();
        fpm.add_pipeline_pass(DcePass);
        let mut fam = FunctionAnalysisManager::new();
        let unverified = fpm.run(verified, f, &mut fam)?;
        let reverified = unverified.verify()?;
        let text = format!("{reverified}");

        assert!(text.contains("%slot = alloca i32"), "{text}");
        assert!(text.contains("store i32 7, ptr %slot"), "{text}");
        assert!(text.contains("ret void"), "{text}");
        assert!(!text.contains("dead0"), "{text}");
        assert!(!text.contains("dead1"), "{text}");
        Ok(())
    })
}

/// llvmkit-specific typed pass-manager smoke test for the upstream
/// `llvm/lib/Passes/PassRegistry.def` entries
/// `FUNCTION_PASS("instsimplify", InstSimplifyPass())` and
/// `FUNCTION_PASS("dce", DCEPass())`.
#[test]
fn scalar_cleanup_passes_have_typed_pipeline_names() {
    let mut fpm = FunctionPassManager::<Brand<'_>, MutatesIr>::new_transform();
    fpm.add_pipeline_pass(InstSimplifyPass);
    fpm.add_pipeline_pass(DcePass);

    assert_eq!(fpm.pipeline_text(), "instsimplify,dce");
}

/// llvmkit-specific typed pipeline smoke test combining
/// `llvm/lib/Transforms/Scalar/InstSimplifyPass.cpp::runImpl`,
/// `llvm/lib/Transforms/Scalar/DCE.cpp::eliminateDeadCode`, and the upstream
/// `PassRegistry.def` function-pass names through `ModuleToFunctionPassAdaptor`.
#[test]
fn instsimplify_and_dce_pipeline_folds_and_erases() -> Result<(), IrError> {
    Module::with_new("scalar-cleanup", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let folded = b.build_int_add::<i32, _, _, _>(
            i32_ty.const_int(40_u32),
            i32_ty.const_int(2_u32),
            "folded",
        )?;
        let dead0 = b.build_int_add::<i32, _, _, _>(
            i32_ty.const_int(1_u32),
            i32_ty.const_int(2_u32),
            "dead0",
        )?;
        let _dead1 = b.build_int_mul::<i32, _, _, _>(dead0, i32_ty.const_int(3_u32), "dead1")?;
        b.build_ret(folded)?;

        let verified = m.verify()?;
        let mut fpm = FunctionPassManager::<_, MutatesIr>::new_transform();
        fpm.add_pipeline_pass(InstSimplifyPass);
        fpm.add_pipeline_pass(DcePass);
        let mut mpm = ModulePassManager::<_, MutatesIr>::new_transform();
        mpm.add_pass(ModuleToFunctionPassAdaptor::new(fpm));
        let mut mam = ModuleAnalysisManager::new();
        let mut fam = FunctionAnalysisManager::new();
        let unverified = mpm.run(verified, &mut mam, &mut fam)?;
        let reverified = unverified.verify()?;
        let text = format!("{reverified}");

        assert!(text.contains("ret i32 42"), "{text}");
        assert!(!text.contains("folded"), "{text}");
        assert!(!text.contains("dead0"), "{text}");
        assert!(!text.contains("dead1"), "{text}");
        Ok(())
    })
}

/// Port of `ConstantFolding.cpp::ConstantFoldLoadFromConstPtr` +
/// `GlobalValue::hasDefinitiveInitializer` through the pass pipeline:
/// instsimplify keeps a load from an interposable (weak) constant global —
/// the linker may select a different definition — while a load from a strong
/// definition still folds away.
#[test]
fn instsimplify_pass_keeps_load_from_interposable_constant_global() -> Result<(), IrError> {
    Module::with_new("instsimplify-weak-global", |m| {
        let i32_ty = m.i32_type();
        let weak = m.add_global_constant("weak_g", i32_ty.as_type(), i32_ty.const_int(42_i32))?;
        weak.set_linkage(&m, Linkage::WeakAny);
        let strong =
            m.add_global_constant("strong_g", i32_ty.as_type(), i32_ty.const_int(7_i32))?;
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let weak_ptr = PointerValue::try_from(weak.as_global_constant_ptr().as_value())?;
        let strong_ptr = PointerValue::try_from(strong.as_global_constant_ptr().as_value())?;
        let w = IntValue::try_from(b.build_load(i32_ty.as_type(), weak_ptr, "w")?)?;
        let s = IntValue::try_from(b.build_load(i32_ty.as_type(), strong_ptr, "s")?)?;
        let sum = b.build_int_add::<i32, _, _, _>(w, s, "sum")?;
        b.build_ret(sum)?;

        let verified = m.verify()?;
        let mut fpm = FunctionPassManager::<_, MutatesIr>::new_transform();
        fpm.add_pipeline_pass(InstSimplifyPass);
        let mut fam = FunctionAnalysisManager::new();
        let unverified = fpm.run(verified, f, &mut fam)?;
        let reverified = unverified.verify()?;
        let text = format!("{reverified}");

        assert!(
            text.contains("%w = load i32, ptr @weak_g"),
            "weak-global load must survive:\n{text}"
        );
        assert!(
            !text.contains("%s = load"),
            "strong-global load must fold away:\n{text}"
        );
        assert!(text.contains("%sum = add i32 %w, 7"), "{text}");
        Ok(())
    })
}
