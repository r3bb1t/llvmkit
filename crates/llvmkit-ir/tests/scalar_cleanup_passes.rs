use llvmkit_ir::{
    Align, Analyses, AtomicLoadConfig, AtomicOrdering, DcePass, IRBuilder, InstSimplifyPass,
    IntValue, IrError, Linkage, Module, NoFolder, PointerValue, SyncScope, Type, Value,
    run_function_pass,
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
        let mut analyses = Analyses::new();
        let unverified = run_function_pass(InstSimplifyPass, verified, f, &mut analyses)?;
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

/// Worklist user-cascade lock (design spec `docs/worklist-erase-safe-cursor-design.md`,
/// Testing → Cascade-tests). InstSimplify seeds the worklist in program order and
/// drains it LIFO, so a dependent user is popped *before* its def: here `%b` pops
/// first and cannot fold (its operand `%a` is not yet constant), then `%a` folds to
/// `3`. The only reason `%b` gets a second, fold-succeeding visit is that
/// `FnPatch::replace_all_uses` re-queues `%a`'s former users (`%b`) after the RAUW.
/// Mirrors `InstSimplifyPass::runImpl`'s use-list-driven re-simplification: folding
/// one instruction must revisit its users so a dependent chain reaches the fixpoint.
/// Without that user push the pass stops one fold short (`%b = add i32 3, 10`),
/// diverging from the restart-scan fixpoint — this test locks the push.
#[test]
fn instsimplify_user_cascade_folds_dependent_add_chain() -> Result<(), IrError> {
    Module::with_new("instsimplify-user-cascade", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        // %a = add i32 1, 2  (used only by %b)
        let a =
            b.build_int_add::<i32, _, _, _>(i32_ty.const_int(1_u32), i32_ty.const_int(2_u32), "a")?;
        // %b = add i32 %a, 10  (used only by the return) — depends on %a.
        let bb = b.build_int_add::<i32, _, _, _>(a, i32_ty.const_int(10_u32), "b")?;
        b.build_ret(bb)?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let unverified = run_function_pass(InstSimplifyPass, verified, f, &mut analyses)?;
        let reverified = unverified.verify()?;
        let text = format!("{reverified}");

        // The whole chain collapses to the single folded constant.
        assert_eq!(
            text,
            concat!(
                "; ModuleID = 'instsimplify-user-cascade'\n",
                "define i32 @f() {\n",
                "entry:\n",
                "  ret i32 13\n",
                "}\n",
            )
        );
        // No add survives: %b must have been revisited and folded, not left as
        // `add i32 3, 10` (the fixpoint-short output when the user push is absent).
        assert!(!text.contains("add"), "{text}");
        assert!(!text.contains("%a"), "{text}");
        assert!(!text.contains("%b"), "{text}");
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
        let mut analyses = Analyses::new();
        let unverified = run_function_pass(DcePass, verified, f, &mut analyses)?;
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

/// llvmkit-specific single-pass driver smoke test combining
/// `llvm/lib/Transforms/Scalar/InstSimplifyPass.cpp::runImpl` and
/// `llvm/lib/Transforms/Scalar/DCE.cpp::eliminateDeadCode`: instsimplify folds
/// the live add to a constant, then dce erases the now-dead chain. Each pass
/// downgrades the module, so it is re-verified before the next runs.
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
        let mut analyses = Analyses::new();
        let after_instsimplify = run_function_pass(InstSimplifyPass, verified, f, &mut analyses)?;
        let reverified = after_instsimplify.verify()?;
        let after_dce = run_function_pass(DcePass, reverified, f, &mut analyses)?;
        let reverified = after_dce.verify()?;
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
        let mut analyses = Analyses::new();
        let unverified = run_function_pass(InstSimplifyPass, verified, f, &mut analyses)?;
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

/// Matches `wouldInstructionBeTriviallyDead` via `LoadInst::isUnordered`: an
/// unused unordered atomic load has no memory-ordering side effects and is
/// removed, while an ordered (monotonic) atomic load and a volatile load are
/// kept.
#[test]
fn dce_removes_unordered_atomic_load_keeps_ordered_and_volatile() -> Result<(), IrError> {
    Module::with_new("dce-loads", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(m.void_type().as_type(), [ptr_ty.as_type()], false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let p: PointerValue = f.param(0)?.try_into()?;
        let unordered =
            AtomicLoadConfig::new(AtomicOrdering::Unordered, SyncScope::System, Align::new(4)?);
        let _u = b.build_int_load_atomic::<i32, _, _>(p, unordered, "u")?;
        let monotonic =
            AtomicLoadConfig::new(AtomicOrdering::Monotonic, SyncScope::System, Align::new(4)?);
        let _mo = b.build_int_load_atomic::<i32, _, _>(p, monotonic, "mo")?;
        let _v = b.build_load_volatile(i32_ty, p, "v")?;
        b.build_ret_void();

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let reverified = run_function_pass(DcePass, verified, f, &mut analyses)?.verify()?;
        let text = format!("{reverified}");

        assert!(
            !text.contains("%u ="),
            "unordered atomic load should be removed:\n{text}"
        );
        assert!(
            text.contains("%mo = load atomic"),
            "ordered atomic load must be kept:\n{text}"
        );
        assert!(
            text.contains("%v = load volatile"),
            "volatile load must be kept:\n{text}"
        );
        Ok(())
    })
}

/// Negative DCE coverage: side-effecting instructions (store, fence, call)
/// are never trivially dead, matching `wouldInstructionBeTriviallyDead`.
#[test]
fn dce_keeps_store_fence_and_call() -> Result<(), IrError> {
    Module::with_new("dce-effects", |m| {
        let i32_ty = m.i32_type();
        let ptr_ty = m.ptr_type(0);
        let void_ty = m.void_type().as_type();
        let sink_ty = m.fn_type(void_ty, Vec::<Type>::new(), false);
        let sink = m.add_function::<(), _>("sink", sink_ty, Linkage::External)?;
        let fn_ty = m.fn_type(void_ty, [ptr_ty.as_type()], false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let p: PointerValue = f.param(0)?.try_into()?;
        b.build_store(i32_ty.const_int(1_u32), p)?;
        b.build_fence(
            AtomicOrdering::SequentiallyConsistent,
            SyncScope::System,
            "",
        )?;
        b.build_call_dyn::<(), _, _, _>(sink, Vec::<Value>::new(), "")?;
        b.build_ret_void();

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let reverified = run_function_pass(DcePass, verified, f, &mut analyses)?.verify()?;
        let text = format!("{reverified}");

        assert!(text.contains("store i32 1"), "store kept:\n{text}");
        assert!(text.contains("fence seq_cst"), "fence kept:\n{text}");
        assert!(text.contains("call void @sink()"), "call kept:\n{text}");
        Ok(())
    })
}

/// Regression (broad-review Critical): InstSimplify must TERMINATE on an
/// ordered atomic load from a constant global. The load folds to the constant
/// but is not trivially dead (its ordering is a side effect), so it is RAUW'd
/// but kept; without upstream's use-empty guard the restart loop re-folded it
/// forever. Mirrors `InstSimplifyPass::runImpl` only simplifying use-having
/// instructions.
#[test]
fn instsimplify_terminates_on_ordered_atomic_load_from_constant() -> Result<(), IrError> {
    Module::with_new("is-atomic", |m| {
        let i32_ty = m.i32_type();
        let g = m.add_global_constant("g", i32_ty.as_type(), i32_ty.const_int(7_i32))?;
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let gp = PointerValue::try_from(g.as_global_constant_ptr().as_value())?;
        let cfg =
            AtomicLoadConfig::new(AtomicOrdering::Monotonic, SyncScope::System, Align::new(4)?);
        let s = b.build_int_load_atomic::<i32, _, _>(gp, cfg, "s")?;
        b.build_ret(s)?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let reverified =
            run_function_pass(InstSimplifyPass, verified, f, &mut analyses)?.verify()?;
        let text = format!("{reverified}");

        // The pass terminated (no hang); the side-effecting load is kept, its
        // use replaced by the folded constant.
        assert!(
            text.contains("load atomic i32"),
            "atomic load kept:\n{text}"
        );
        assert!(
            text.contains("ret i32 7"),
            "use folded to constant:\n{text}"
        );
        Ok(())
    })
}
