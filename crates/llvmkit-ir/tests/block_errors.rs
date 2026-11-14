//! Each caller-blamed [`BlockError`] variant is what the entry that raises it
//! returns.
//!
//! **No upstream counterpart.** Upstream's `Function` block-list splice and
//! `BasicBlock::splitBasicBlock*` `assert` these conditions, so no upstream
//! unit test can observe them. What these tests pin is llvmkit's typed
//! refusal. `BlockError::AnchorNotInBlock` and `ParentListMissingBlock` have
//! none: they report a broken llvmkit invariant, which no public entry can
//! provoke (`tests/typed_error_coverage.rs` lists the reason).

use llvmkit_ir::{
    BasicBlock, Blame, BlockError, Dyn, IrBuilder, IrError, Linkage, Type, module_new,
};

/// The refusal an `Err` carries, or a panic naming the entry that succeeded.
fn refusal<T>(result: Result<T, IrError>, entry: &str) -> IrError {
    match result {
        Ok(_) => panic!("{entry} must refuse"),
        Err(error) => error,
    }
}

/// A function refuses a block of another function, both as a block to move
/// to its end and as a block to construct into, and refuses an insertion
/// handle for a block that is already terminated.
#[test]
fn a_function_refuses_a_block_that_is_not_its_own() -> Result<(), IrError> {
    let m = module_new!("block-errors-function")?;
    let fn_ty = m.function_type(m.void_type(), Vec::<Type<'_, _>>::new())?;
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let g = m.add_function_dyn("g", fn_ty, Linkage::External)?;
    let f_entry = m.view(f).append_basic_block(&m, "entry");
    let f_entry_value = f_entry.to_erased();
    let g_entry = m.view(g).append_basic_block(&m, "entry");
    let g_entry_value = g_entry.to_erased();

    let err = refusal(
        m.view(f).basic_block_for_construction(&m, g_entry_value),
        "construction into @f of a block of @g",
    );
    assert_eq!(err, IrError::from(BlockError::BlockNotInFunction));
    let err = refusal(
        m.view(f).move_basic_block_to_end(&m, g_entry),
        "moving a block of @g to the end of @f",
    );
    assert_eq!(err, IrError::from(BlockError::BlockNotInFunction));
    assert_eq!(err.blame(), Blame::UsageError);

    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(f_entry)
        .ret_void()?;
    let err = refusal(
        m.view(f).basic_block_for_construction(&m, f_entry_value),
        "construction into a terminated block",
    );
    assert_eq!(err, IrError::from(BlockError::InsertionIntoTerminatedBlock));
    Ok(())
}

/// A terminated block that belongs to no function refuses to split: its new
/// block would have no function to join. A `br` terminates it, as a branch
/// needs no function to check against.
#[test]
fn an_orphan_block_refuses_to_split() -> Result<(), IrError> {
    let m = module_new!("block-errors-orphan")?;
    let target = BasicBlock::<Dyn, _, _>::create_orphan(&m, "target");
    let orphan = BasicBlock::<Dyn, _, _>::create_orphan(&m, "orphan");
    let (terminated, _) = IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(orphan)
        .br(target.id())?;
    let ret = terminated
        .instructions()
        .next()
        .expect("the orphan holds its `ret`");
    let err = refusal(
        terminated.split_at(&m, &ret, "orphan.split"),
        "splitting an orphan block",
    );
    assert_eq!(err, IrError::from(BlockError::SplitOrphanBlock));
    Ok(())
}

/// A `Dyn`-marker builder positioned in an orphan block refuses a `ret`: it
/// checks a return against the function's return type, and there is no
/// function. This used to be an `unreachable!` that a builder positioned at
/// [`BasicBlock::create_orphan`]'s block reached.
///
/// **No upstream counterpart**: upstream's `ReturnInst` checks no type, so
/// `IRBuilder::CreateRetVoid` builds the `ret` regardless.
#[test]
fn a_dyn_return_in_an_orphan_block_is_refused() -> Result<(), IrError> {
    let m = module_new!("block-errors-orphan-ret")?;
    let orphan = BasicBlock::<Dyn, _, _>::create_orphan(&m, "orphan");
    let err = refusal(
        IrBuilder::new_for::<Dyn>(&m)
            .position_at_end(orphan)
            .ret_void(),
        "a Dyn `ret void` in an orphan block",
    );
    assert_eq!(
        err,
        IrError::from(llvmkit_ir::BuilderError::ReturnOutsideFunction)
    );
    let orphan = BasicBlock::<Dyn, _, _>::create_orphan(&m, "orphan.value");
    let err = refusal(
        IrBuilder::new_for::<Dyn>(&m)
            .position_at_end(orphan)
            .ret(m.i32_type().const_int(0_i32)),
        "a Dyn `ret i32` in an orphan block",
    );
    assert_eq!(
        err,
        IrError::from(llvmkit_ir::BuilderError::ReturnOutsideFunction)
    );
    Ok(())
}
