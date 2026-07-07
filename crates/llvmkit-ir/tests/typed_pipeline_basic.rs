use llvmkit_ir::{
    DominatorTreeAnalysis, FunctionAnalysisManager, FunctionView, IRBuilder, IrError, Linkage,
    Module, ModuleBrand, Type,
};

use std::cell::RefCell;
use std::rc::Rc;

use llvmkit_ir::{
    CFGAnalyses, MutatesIr, NoFolder, PreserveSet, PreservedAnalyses, PreservesVerification,
    TypedFunctionPass, TypedFunctionPassContext, function_pipeline,
};

/// Brand-generic mirror of `llvm/unittests/IR/PassManagerTest.cpp::TEST(PassManagerTest,
/// Basic)`'s analysis-registration/getResult flow: an analysis must be usable from code
/// that is generic over the module brand, not just the default brand.
fn domtree_via_generic_brand<'ctx, B: ModuleBrand + 'ctx>(
    function: FunctionView<'ctx, B>,
    fam: &mut FunctionAnalysisManager<'ctx, B>,
) -> Result<bool, IrError> {
    fam.ensure_registered_default::<DominatorTreeAnalysis>();
    let entry = function.entry_block();
    let dt = fam.get_result::<DominatorTreeAnalysis, _>(function)?;
    // BasicBlockView implements DominatorTreeBlock (dominator_tree.rs lines 44-132),
    // so the entry view queries reachability directly.
    Ok(match entry {
        Some(bb) => dt.is_reachable_from_entry(bb),
        None => false,
    })
}

/// Same upstream anchor; drives the generic helper through a real module.
#[test]
fn dominator_tree_analysis_is_brand_generic() -> Result<(), IrError> {
    Module::with_new("brand-generic-domtree", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        b.build_ret(i32_ty.const_int(1_u32))?;
        let _verified = m.verify()?;

        let mut fam = FunctionAnalysisManager::new();
        assert!(domtree_via_generic_brand(f.into(), &mut fam)?);
        Ok(())
    })
}

/// Shared IR-builder helper: a single-block `i32 @f()` whose entry returns a
/// constant. Modelled on `tests/scalar_cleanup_passes.rs`'s ret-only fixtures.
fn build_ret_i32<'ctx, B: ModuleBrand + 'ctx>(
    m: &Module<'ctx, B, llvmkit_ir::Unverified>,
) -> Result<FunctionView<'ctx, B>, IrError> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, Vec::<Type<'ctx, B>>::new(), false);
    let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block(m, "entry");
    let b = IRBuilder::new_for::<i32>(m).position_at_end(entry);
    b.build_ret(i32_ty.const_int(1_u32))?;
    Ok(f.into())
}

/// Shared IR-builder helper: `i32 @f()` with one unused constant `add` named
/// `dead` before the terminator. Uses `NoFolder` so the constant add is not
/// folded away at construction time (matches
/// `tests/scalar_cleanup_passes.rs`'s dead-instruction idiom).
fn build_dead_add_then_ret<'ctx, B: ModuleBrand + 'ctx>(
    m: &Module<'ctx, B, llvmkit_ir::Unverified>,
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

/// Mirrors `llvm/unittests/IR/PassManagerTest.cpp::TEST(PassManagerTest, Basic)`
/// pass-sequencing on the typed pipeline path: two passes run in order and the
/// pipeline's aggregate PreservedAnalyses is their intersection.
struct LogPass {
    log: Rc<RefCell<Vec<&'static str>>>,
    tag: &'static str,
}

impl<'ctx, B: ModuleBrand + 'ctx> TypedFunctionPass<'ctx, B> for LogPass {
    type Effect = PreservesVerification;
    type Requires = ();
    type MinPreserves = ();
    const NAME: &'static str = "log";

    fn run(
        &mut self,
        _cx: &mut TypedFunctionPassContext<'_, '_, 'ctx, B, (), PreservesVerification>,
    ) -> Result<PreservedAnalyses, IrError> {
        self.log.borrow_mut().push(self.tag);
        Ok(PreservedAnalyses::all())
    }
}

/// Same upstream anchor: a read-only pipeline of read-only passes keeps the
/// module Verified — the effect is derived, not declared.
#[test]
fn read_only_pipeline_returns_verified_module() -> Result<(), IrError> {
    Module::with_new("typed-ro", |m| {
        let f = build_ret_i32(&m)?;
        let verified = m.verify()?;
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut pipe = function_pipeline((
            LogPass {
                log: log.clone(),
                tag: "a",
            },
            LogPass {
                log: log.clone(),
                tag: "b",
            },
        ));
        let mut fam = FunctionAnalysisManager::new();
        // The whole point: this binding type-checks as Verified.
        let _still_verified: Module<'_, _, llvmkit_ir::Verified> =
            pipe.run(verified, f, &mut fam)?;
        assert_eq!(*log.borrow(), vec!["a", "b"]);
        Ok(())
    })
}

/// Local `MutatesIr` transform stand-in for `DcePass`, which is not yet a
/// `TypedFunctionPass` (its migration is Task 7). The typed-pipeline test only
/// needs one transform member so the pipeline's derived effect joins to
/// `MutatesIr`; the erase itself is exercised by the erased-path DCE tests in
/// `tests/scalar_cleanup_passes.rs`.
struct MutatingNoop;

impl<'ctx, B: ModuleBrand + 'ctx> TypedFunctionPass<'ctx, B> for MutatingNoop {
    type Effect = MutatesIr;
    type Requires = ();
    type MinPreserves = ();
    const NAME: &'static str = "mutating-noop";

    fn run(
        &mut self,
        _cx: &mut TypedFunctionPassContext<'_, '_, 'ctx, B, (), MutatesIr>,
    ) -> Result<PreservedAnalyses, IrError> {
        Ok(PreservedAnalyses::none())
    }
}

/// Mirrors the same PassManagerTest sequencing for a mixed pipeline: one
/// transform member joins the pipeline effect to MutatesIr, so run() returns
/// an Unverified module that must be re-verified (D8). `DcePass` will replace
/// `MutatingNoop` once it is migrated to `TypedFunctionPass` in Task 7.
#[test]
fn mixed_pipeline_returns_unverified_module() -> Result<(), IrError> {
    Module::with_new("typed-mixed", |m| {
        let f = build_dead_add_then_ret(&m)?;
        let verified = m.verify()?;
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut pipe = function_pipeline((
            LogPass {
                log: log.clone(),
                tag: "ro",
            },
            MutatingNoop,
        ));
        let mut fam = FunctionAnalysisManager::new();
        // The derived effect is MutatesIr: run() yields Module<Unverified>, so
        // this binding only type-checks because the transform member downgraded
        // the pipeline's output typestate.
        let unverified: Module<'_, _, llvmkit_ir::Unverified> = pipe.run(verified, f, &mut fam)?;
        let reverified = unverified.verify()?;
        assert_eq!(*log.borrow(), vec!["ro"]);
        let text = format!("{reverified}");
        assert!(text.contains("%dead"), "{text}");
        Ok(())
    })
}

/// Mirrors `PassManagerTest.cpp` analysis-getResult flow with zero manual
/// registration: `Requires` prefetch registers and computes DominatorTree, and
/// the typed context accessor is infallible.
struct NeedsDomTree {
    saw_entry_reachable: Rc<RefCell<Option<bool>>>,
}

impl<'ctx, B: ModuleBrand + 'ctx> TypedFunctionPass<'ctx, B> for NeedsDomTree {
    type Effect = PreservesVerification;
    type Requires = (DominatorTreeAnalysis,);
    type MinPreserves = ();
    const NAME: &'static str = "needs-domtree";

    fn run(
        &mut self,
        cx: &mut TypedFunctionPassContext<
            '_,
            '_,
            'ctx,
            B,
            (DominatorTreeAnalysis,),
            PreservesVerification,
        >,
    ) -> Result<PreservedAnalyses, IrError> {
        let dt = cx.analysis::<DominatorTreeAnalysis, _>(); // no IrResult, no unwrap
        let reachable = cx
            .function()
            .entry_block()
            .map(|bb| dt.is_reachable_from_entry(bb));
        *self.saw_entry_reachable.borrow_mut() = reachable;
        Ok(PreservedAnalyses::all())
    }
}

#[test]
fn requires_prefetch_makes_analysis_access_infallible() -> Result<(), IrError> {
    Module::with_new("typed-requires", |m| {
        let f = build_ret_i32(&m)?;
        let verified = m.verify()?;
        let seen = Rc::new(RefCell::new(None));
        let mut pipe = function_pipeline((NeedsDomTree {
            saw_entry_reachable: seen.clone(),
        },));
        let mut fam = FunctionAnalysisManager::new(); // note: NO register_pass call
        let _v = pipe.run(verified, f, &mut fam)?;
        assert_eq!(*seen.borrow(), Some(true));
        Ok(())
    })
}

/// Mirrors the CFGAnalyses-preservation invalidation rule of
/// `DominatorTree::invalidate` (ported in dominator_tree.rs): a pass whose
/// MinPreserves declares PreserveSet<CFGAnalyses> keeps the cached tree alive
/// even though its runtime return is none().
struct UnderReportingPass;

impl<'ctx, B: ModuleBrand + 'ctx> TypedFunctionPass<'ctx, B> for UnderReportingPass {
    type Effect = PreservesVerification;
    type Requires = (DominatorTreeAnalysis,);
    type MinPreserves = (PreserveSet<CFGAnalyses>,);
    const NAME: &'static str = "under-reporting";

    fn run(
        &mut self,
        _cx: &mut TypedFunctionPassContext<
            '_,
            '_,
            'ctx,
            B,
            (DominatorTreeAnalysis,),
            PreservesVerification,
        >,
    ) -> Result<PreservedAnalyses, IrError> {
        Ok(PreservedAnalyses::none()) // "forgets" the CFG set; the bound unions it back
    }
}

#[test]
fn min_preserves_bound_is_unioned_into_runtime_result() -> Result<(), IrError> {
    Module::with_new("typed-minpreserves", |m| {
        let f = build_ret_i32(&m)?;
        let verified = m.verify()?;
        let mut pipe = function_pipeline((UnderReportingPass,));
        let mut fam = FunctionAnalysisManager::new();
        let _v = pipe.run(verified, f, &mut fam)?;
        // DominatorTree survives because CFGAnalyses was force-preserved.
        assert!(
            fam.get_cached_result::<DominatorTreeAnalysis, _>(f)
                .is_some(),
            "MinPreserves union must keep the CFG-set-preserved DominatorTree cached"
        );
        Ok(())
    })
}
