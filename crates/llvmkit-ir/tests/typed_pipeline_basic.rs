use llvmkit_ir::{
    DominatorTreeAnalysis, FunctionAnalysisManager, FunctionView, IRBuilder, IrError, Linkage,
    Module, ModuleBrand, Type,
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
