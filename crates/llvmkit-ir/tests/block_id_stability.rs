//! A [`BlockId`] stays valid across a block replace-all-uses.
//!
//! The C++ analogue is `BasicBlock::replaceAllUsesWith(Old, New)`: every
//! terminator that named `Old` as a successor is retargeted to `New`. Any
//! client that keeps a side map keyed by `BasicBlock *` — and a lifter always
//! does, mapping guest addresses to blocks — has to *hand-migrate* that map
//! afterwards, because the pointers it holds may now name a block nothing
//! reaches, or a block about to be deleted.
//!
//! llvmkit's ids are not pointers. A [`BlockId`] is a `(module tag, arena
//! slot)` pair minted once and resolved on demand, so redirecting edges is a
//! mutation of *terminators*, not of the blocks themselves: a map keyed by
//! `BlockId` needs no migration at all. This file pins that, in the strongest
//! form available — the ids stored *before* the edit are not merely still
//! resolvable afterwards, they still **drive the mutation API**, which is what
//! actually matters to a client that keeps editing.
//!
//! ## Upstream provenance
//!
//! Closest upstream behaviour: `BasicBlock::replaceAllUsesWith` /
//! `Value::replaceAllUsesWith` on a block value, and the `ValueMap` /
//! `ValueToValueMapTy` bookkeeping every LLVM transform that reshapes a CFG
//! carries alongside it (`llvm/lib/Transforms/Utils/CloneFunction.cpp`,
//! `BasicBlockUtils.cpp`). llvmkit-specific in the guarantee it locks: LLVM
//! has no id layer to make the claim about.

use llvmkit_ir::{
    Analyses, BlockId, Dyn, FnCx, FnReport, FunctionId, FunctionPass, IRBuilder, IntPredicate,
    IntValue, IrError, IrResult, Linkage, Module, ModuleBrand, ReshapeCfg, Unverified, module_new,
};

/// Everything the fixture hands back: the function plus the four block ids a
/// client would have cached before any edit ran.
struct Fixture<B: ModuleBrand> {
    function: FunctionId<Dyn, B>,
    left: BlockId<Dyn, B>,
    right: BlockId<Dyn, B>,
    old: BlockId<Dyn, B>,
    new: BlockId<Dyn, B>,
}

/// ```text
/// entry: %c = icmp eq i32 %n, 0
///        br i1 %c, label %left, label %right
/// left:  br label %old
/// right: br label %old
/// old:   ret i32 3
/// new:   ret i32 7
/// ```
///
/// `old` has two predecessors and `new` has none, so redirecting both `br`s is
/// exactly a block replace-all-uses of `old` by `new`.
fn build<'ctx, B: ModuleBrand + 'ctx>(m: &'ctx Module<B, Unverified>) -> IrResult<Fixture<B>> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let function = m.add_function_dyn("f", fn_ty, Linkage::External)?;

    let entry = m.view(function).append_basic_block(m, "entry");
    let left = m.view(function).append_basic_block(m, "left");
    let right = m.view(function).append_basic_block(m, "right");
    let old = m.view(function).append_basic_block(m, "old");
    let new = m.view(function).append_basic_block(m, "new");

    // Capture the ids up front — this is the client's side map.
    let (left_id, right_id, old_id, new_id) = (left.id(), right.id(), old.id(), new.id());

    let bo = IRBuilder::new_for::<Dyn>(m).position_at_end(old);
    bo.build_ret(i32_ty.const_int(3_i32))?;
    let bn = IRBuilder::new_for::<Dyn>(m).position_at_end(new);
    bn.build_ret(i32_ty.const_int(7_i32))?;
    let bl = IRBuilder::new_for::<Dyn>(m).position_at_end(left);
    bl.build_br(old_id)?;
    let br = IRBuilder::new_for::<Dyn>(m).position_at_end(right);
    br.build_br(old_id)?;

    let be = IRBuilder::new_for::<Dyn>(m).position_at_end(entry);
    let n: IntValue<'_, i32, _> = m.view(function).param(0)?.try_into()?;
    let c = be.build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, n, 0_i32, "c")?;
    be.build_cond_br(c, left_id, right_id)?;

    Ok(Fixture {
        function,
        left: left_id,
        right: right_id,
        old: old_id,
        new: new_id,
    })
}

/// Retarget every listed block's unconditional `br` onto `new_to`. Driven
/// entirely by ids the caller minted before the pass ran.
struct RedirectAllPreds<B: ModuleBrand> {
    from: Vec<BlockId<Dyn, B>>,
    new_to: BlockId<Dyn, B>,
}

impl<B: ModuleBrand> FunctionPass<B> for RedirectAllPreds<B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "redirect-all-preds";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let reshape = cx.mutate();
        for &from in &self.from {
            reshape.edit_br(from)?.redirect(self.new_to, &[])?;
        }
        Ok(reshape.done())
    }
}

/// The headline: a `BlockId` minted before a block replace-all-uses is still
/// good afterwards -- it resolves, it names the same block, and it still works
/// as an argument to the mutation API. No map migration anywhere.
#[test]
fn block_ids_survive_a_block_replace_all_uses() -> Result<(), IrError> {
    let m = module_new!("block-rauw")?;
    let Fixture {
        function,
        left,
        right,
        old,
        new,
    } = build(&m)?;
    let verified = m.verify()?;

    // --- the edit: every use of `old` as a branch target becomes `new` ---
    let mut analyses = Analyses::new();
    let edited = llvmkit_ir::run_function_pass(
        RedirectAllPreds {
            from: vec![left, right],
            new_to: new,
        },
        verified,
        function,
        &mut analyses,
    )?;

    let printed = format!("{edited}");
    assert_eq!(
        printed.matches("br label %new").count(),
        2,
        "both predecessors must now target %new:\n{printed}"
    );
    assert!(
        !printed.contains("br label %old"),
        "nothing may still branch to %old:\n{printed}"
    );
    edited.verify_borrowed()?;

    // --- the point: every cached id is untouched by the edit ---
    //
    // `old` is now unreachable and `new` has gained two predecessors, but
    // neither block moved, and the side map built before the edit still names
    // exactly what it named then. This is the migration the C++ client has to
    // perform by hand and llvmkit's client does not.
    for (id, name) in [(left, "left"), (right, "right"), (old, "old"), (new, "new")] {
        assert!(
            edited.try_view(id).is_some(),
            "{name}'s id must still resolve after the edit"
        );
        assert_eq!(
            edited.view(id).to_erased().name().as_deref(),
            Some(name),
            "{name}'s id must still name the same block"
        );
    }

    // --- and they are still *usable*, not merely resolvable ---
    //
    // Resolution alone would be a weak claim (a dangling `BasicBlock *` also
    // "resolves"). Drive a second edit entirely from the pre-edit ids: send
    // `left` back to `old`.
    let reverified = edited.verify()?;
    let again = llvmkit_ir::run_function_pass(
        RedirectAllPreds {
            from: vec![left],
            new_to: old,
        },
        reverified,
        function,
        &mut analyses,
    )?;
    let printed = format!("{again}");
    assert!(
        printed.contains("br label %old"),
        "a pre-edit id must still drive the mutation API:\n{printed}"
    );
    assert_eq!(printed.matches("br label %new").count(), 1, "{printed}");
    again.verify_borrowed()?;
    Ok(())
}
