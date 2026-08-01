//! View-layer iterators do not borrow the receiver they came from.
//!
//! **No upstream counterpart.** This is a Rust-API law with nothing to mirror
//! in LLVM: C++ iterators are raw pointers into an intrusive list, so the
//! question of whether `BasicBlock::instructions()` borrows the `BasicBlock`
//! does not arise there.
//!
//! Every view-layer iterator in this crate that snapshots its contents up
//! front carries a `use<..>` bound that keeps the `&self` lifetime out of the
//! returned opaque type. Without it, edition 2024's capture rules put that
//! lifetime in anyway, and the natural composition
//!
//! ```text
//! function.basic_blocks().flat_map(|block| block.instructions())
//! ```
//!
//! fails to borrow-check (E0515) even though the iterator holds nothing
//! belonging to `block` — forcing callers into nested loops with labeled
//! breaks. Iterators that genuinely borrow (`AttributeSet::iter`, which yields
//! `&Attribute`; `Cfg::edges`; `PassPipeline::pass_names`) keep the borrow and
//! are deliberately not covered here.
//!
//! These tests are compile-time claims first: if a `use<..>` bound is dropped,
//! the file stops compiling rather than failing an assertion.

use llvmkit_ir::{
    Dyn, IRBuilder, InstructionKind, InstructionView, IntValue, IrError, IsValue, Linkage,
    ModuleBrand, NoFolder, Value, module_new,
};

/// Build `@f` with two blocks and a phi, so there is something to walk:
///
/// ```text
/// entry:            %sum = add i32 %a, %a
///                   br label %join(%sum)
/// join(%p: i32):    %doubled = add i32 %p, %p
///                   ret i32 %doubled
/// ```
///
/// Returns the module plus the instruction names in program order — the phi
/// is `join`'s block parameter, so it heads the second block.
fn two_block_function<B: ModuleBrand>(m: &llvmkit_ir::Module<B>) -> Result<Vec<String>, IrError> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty.as_type(), [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(m, "entry");

    let scaffold = IRBuilder::new_for::<Dyn>(m);
    let (join, params) =
        scaffold.append_block_with_params(m.view(f), &[i32_ty.as_type()], "join")?;
    let join_label = join.id();

    let b = IRBuilder::with_folder(m, NoFolder).position_at_end(entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let sum = b.build_int_add::<i32, _, _, _>(a, a, "sum")?;
    let sum_erased = b.view(sum).into_erased();
    b.build_br_with_args(join_label, &[sum_erased])?;

    let b = IRBuilder::with_folder(m, NoFolder).position_at_end(join);
    let p: IntValue<'_, i32, _> = params[0].try_into()?;
    let doubled = b.build_int_add::<i32, _, _, _>(p, p, "doubled")?;
    let doubled_erased = b.view(doubled).into_erased();
    b.build_ret(doubled_erased)?;

    // The phi carries no name of its own (it is an anonymous block parameter),
    // so program order is: entry's `sum`, entry's `br`, join's phi, `doubled`,
    // `ret`. Only the named ones are asserted on below.
    Ok(vec!["sum".to_string(), "doubled".to_string()])
}

/// `flat_map`ping `instructions()` across a function's blocks compiles, and
/// yields every instruction of every block in program order.
///
/// This is the exact composition that E0515 used to reject. No upstream
/// counterpart — see the module docs.
#[test]
fn instructions_flat_map_across_blocks_compiles_and_is_ordered() -> Result<(), IrError> {
    let m = module_new!("view-iter-flatmap")?;
    let expected_named = two_block_function(&m)?;
    let f = m.as_view().functions().next().expect("@f exists");

    // The composition under test. Binding it proves the iterator outlives the
    // `block` it came from — with the borrow captured this would not compile.
    let walk: Vec<InstructionView<'_, _>> = f
        .basic_blocks()
        .flat_map(|block| block.instructions())
        .collect();

    let per_block: Vec<usize> = f.basic_blocks().map(|b| b.instructions().len()).collect();
    assert_eq!(
        walk.len(),
        per_block.iter().sum::<usize>(),
        "flat_map must yield every instruction of every block"
    );
    assert_eq!(per_block.len(), 2, "entry and join");

    let named: Vec<String> = walk.iter().filter_map(|i| i.name()).collect();
    assert_eq!(named, expected_named, "program order across blocks");

    // Block-by-block order must agree with the flattened order.
    let nested: Vec<InstructionView<'_, _>> = {
        let mut collected = Vec::new();
        for block in f.basic_blocks() {
            for instruction in block.instructions() {
                collected.push(instruction);
            }
        }
        collected
    };
    assert_eq!(
        walk.iter().map(|i| i.slot()).collect::<Vec<_>>(),
        nested.iter().map(|i| i.slot()).collect::<Vec<_>>(),
    );
    Ok(())
}

/// An `instructions()` iterator outlives the block view that produced it.
///
/// The block is dropped before the iterator is consumed; this only type-checks
/// because the `use<..>` bound excludes the `&self` lifetime. No upstream
/// counterpart — see the module docs.
#[test]
fn instructions_iterator_outlives_its_block_view() -> Result<(), IrError> {
    let m = module_new!("view-iter-outlives")?;
    two_block_function(&m)?;
    let f = m.as_view().functions().next().expect("@f exists");

    let iterator = {
        let block = f.basic_blocks().next().expect("entry block");
        block.instructions()
        // `block` is dropped here, before a single item is pulled.
    };
    assert_eq!(iterator.count(), 2, "entry holds `sum` and its `br`");
    Ok(())
}

/// The same law for the phi family: `incomings()` outlives the phi handle, and
/// composes under `flat_map` across a block's phis.
///
/// No upstream counterpart — see the module docs.
#[test]
fn phi_incomings_iterator_outlives_its_phi_handle() -> Result<(), IrError> {
    let m = module_new!("view-iter-phi")?;
    two_block_function(&m)?;
    let f = m.as_view().functions().next().expect("@f exists");
    let join = f.basic_blocks().nth(1).expect("join block");

    let pairs: Vec<(Value<'_, _>, _)> = join
        .instructions()
        .filter_map(|instruction| match instruction.kind()? {
            InstructionKind::Phi(phi) => Some(phi),
            _ => None,
        })
        .flat_map(|phi| phi.incomings())
        .collect();
    assert_eq!(pairs.len(), 1, "join's parameter phi has one incoming edge");
    Ok(())
}
