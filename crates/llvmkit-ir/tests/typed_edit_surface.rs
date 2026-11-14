//! Typed terminator edit-handle surface (`FnReshape::edit_*`).
//!
//! These tests exercise the typed edit handles introduced alongside the
//! invoke/callbr edge redirects: narrowing a terminator into a handle whose
//! *type* fixes which edge ops exist, then driving each op and re-verifying the
//! emitted IR. The compile-time half of the guarantee — that a handle simply
//! has no `remove_*` where an edge is not removable — lives in the
//! `tests/compile_fail/` fixtures; here we assert the *runtime* behavior of the
//! ops that do exist.

use std::cell::RefCell;
use std::rc::Rc;

use llvmkit_ir::{
    Analyses, BasicBlock, BasicBlockLabel, BlockId, CfgEditError, Dyn, EditedTerminator, FnCx,
    FnReport, FunctionId, FunctionPass, InsertPoint, IntPredicate, IntValue, IrBuilder, IrError,
    IrResult, LayoutToken, Linkage, Module, ModuleBrand, PositionedBefore, ReshapeCfg, TermEdit,
    Value, ValueId, module_new, run_function_pass,
};

// Fixture return-type aliases. These keep the the emitters helper signatures under
// clippy's `type_complexity` threshold without an `#[allow]` (the repo bans
// `#[allow(...)]`).

/// Return of `build_callbr_caller`: the caller function and the `%new` `Dyn`
/// label a redirect can aim at.
type CallerFixture<B> = (FunctionId<(), B>, BlockId<Dyn, B>);

/// Return of `build_invoke_caller`: the caller function, the plain `%new`
/// label the *normal* edge can be redirected to, and the EH-pad `%new.pad`
/// label the *unwind* edge can be redirected to.
///
/// They cannot be one block. `Verifier::visitInvokeInst` requires an unwind
/// destination to be an EH pad, and `visitEHPadPredecessors` requires a
/// `landingpad`'s block to be jumped to *only* by an invoke's unwind edge — so
/// one block can serve one redirect or the other, never both.
type InvokeCallerFixture<B> = (FunctionId<(), B>, BlockId<Dyn, B>, BlockId<Dyn, B>);

/// Return of `build_switch_fn`: the function plus the `case0` and `new` `Dyn`
/// labels.
type SwitchFixture<B> = (FunctionId<i32, B>, BlockId<Dyn, B>, BlockId<Dyn, B>);

/// Return of `build_switch_bogus_fn`: the function, a non-case (`bogus`) `Dyn`
/// label, the `new` `Dyn` label (which carries a head-phi), and a valid phi
/// seed value id for `new`.
type SwitchBogusFixture<B> = (
    FunctionId<i32, B>,
    BlockId<Dyn, B>,
    BlockId<Dyn, B>,
    ValueId<B>,
);

// ---------------------------------------------------------------------------
// invoke — NEW capability: redirect the normal / unwind edge.
// ---------------------------------------------------------------------------

/// A `ReshapeCfg` pass that narrows the entry block's `invoke` and redirects
/// one of its two edges (chosen by `which`) onto `new_to`.
struct RedirectInvokeEdge<B: ModuleBrand> {
    which: InvokeArm,
    new_to: BlockId<Dyn, B>,
}

#[derive(Clone, Copy)]
enum InvokeArm {
    Normal,
    Unwind,
}

impl<B: ModuleBrand> FunctionPass<B> for RedirectInvokeEdge<B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "redirect-invoke-edge";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport<B>>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let reshape = cx.mutate();
        let entry = reshape
            .function()
            .entry_block()
            .expect("definition has entry");
        let invoke = reshape.edit_invoke(entry.id())?;
        match self.which {
            InvokeArm::Normal => invoke.redirect_normal(self.new_to, &[])?,
            InvokeArm::Unwind => invoke.redirect_unwind(self.new_to, &[])?,
        }
        Ok(reshape.done())
    }
}

/// Build `void @caller()` with an `invoke void @callee() to label %normal
/// unwind label %unwind`, plus an unreferenced `%new` block that a redirect can
/// aim at. Returns the caller and the `%new` `Dyn` label.
fn build_invoke_caller<'ctx, B: ModuleBrand + 'ctx>(
    key: &mut LayoutToken<'ctx, B>,
) -> IrResult<InvokeCallerFixture<B>> {
    let m = key.module();
    let callee = m
        .add_typed_function::<(), (), _>("callee", Linkage::External)?
        .as_function();
    let caller = m
        .add_typed_function::<(), (), _>("caller", Linkage::External)?
        .as_function();

    m.view(caller)
        .set_personality_fn(m, m.ptr_type_in::<0>().const_null())?;
    let entry = m.view(caller).append_basic_block(m, "entry");
    let normal = m.view(caller).append_basic_block(m, "normal");
    let unwind = m.view(caller).append_basic_block(m, "unwind");
    let new = m.view(caller).append_basic_block(m, "new");
    let new_pad = m.view(caller).append_basic_block(m, "new.pad");
    // Capture the labels before `position_at_end` consumes the block handles.
    let normal_lbl = normal.id();
    let unwind_lbl = unwind.id();
    let new_dyn: BasicBlockLabel<'_, Dyn, _> = new.to_erased().try_into()?;
    let new_dyn = new_dyn.id();
    let new_pad_dyn: BasicBlockLabel<'_, Dyn, _> = new_pad.to_erased().try_into()?;
    let new_pad_dyn = new_pad_dyn.id();

    let bn = IrBuilder::new_for::<()>(key).position_at_end(normal);
    bn.ret_void();
    // `Verifier::visitInvokeInst` requires the unwind destination to be an EH
    // pad, and `visitLandingPadInst` requires the `personality` set above.
    let bu = IrBuilder::new_for::<()>(key).position_at_end(unwind);
    let _closed = bu.landingpad(m.i32_type().as_type(), true, "lp")?.finish();
    bu.ret_void();
    let bnew = IrBuilder::new_for::<()>(key).position_at_end(new);
    bnew.ret_void();
    // The unwind-redirect target: an EH pad of its own, with no predecessor
    // until the redirect gives it one.
    let bnew_pad = IrBuilder::new_for::<()>(key).position_at_end(new_pad);
    let _closed = bnew_pad
        .landingpad(m.i32_type().as_type(), true, "lp.new")?
        .finish();
    bnew_pad.ret_void();

    let b = IrBuilder::new_for::<()>(key).position_at_end(entry);
    let _ = b.invoke_dyn(
        m.view(callee),
        Vec::<Value<'_, _>>::new(),
        normal_lbl,
        unwind_lbl,
        "",
    )?;
    Ok((caller, new_dyn, new_pad_dyn))
}

/// `edit_invoke(..).redirect_normal(new, [])` retargets ONLY the normal edge;
/// the unwind edge is untouched, and the output re-verifies.
#[test]
fn invoke_redirect_normal_retargets_normal_edge() -> Result<(), IrError> {
    let mut module = module_new!("invoke-redirect-normal")?;
    let mut key = module.layout_token();
    let (caller, new_dyn, _new_pad_dyn) = build_invoke_caller(&mut key)?;
    let verified = module.verify()?;
    let mut analyses = Analyses::new();
    let pass = RedirectInvokeEdge {
        which: InvokeArm::Normal,
        new_to: new_dyn,
    };
    let out = run_function_pass(pass, verified, caller, &mut analyses)?;
    let reverified = out.verify().expect("invoke redirect output must re-verify");
    let printed = format!("{reverified}");
    assert!(
        printed.contains("to label %new unwind label %unwind"),
        "normal edge must now target %new, unwind untouched, got:\n{printed}"
    );
    Ok(())
}

/// `edit_invoke(..).redirect_unwind(new, [])` retargets ONLY the unwind edge.
#[test]
fn invoke_redirect_unwind_retargets_unwind_edge() -> Result<(), IrError> {
    let mut module = module_new!("invoke-redirect-unwind")?;
    let mut key = module.layout_token();
    let (caller, _new_dyn, new_pad_dyn) = build_invoke_caller(&mut key)?;
    let verified = module.verify()?;
    let mut analyses = Analyses::new();
    let pass = RedirectInvokeEdge {
        which: InvokeArm::Unwind,
        new_to: new_pad_dyn,
    };
    let out = run_function_pass(pass, verified, caller, &mut analyses)?;
    let reverified = out.verify().expect("invoke redirect output must re-verify");
    let printed = format!("{reverified}");
    assert!(
        printed.contains("to label %normal unwind label %new.pad"),
        "unwind edge must now target %new.pad, normal untouched, got:\n{printed}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// callbr — redirect the default / an indirect edge.
// ---------------------------------------------------------------------------

/// Which edge of a `callbr` a [`RedirectCallBrEdge`] retargets.
#[derive(Clone, Copy)]
enum CallBrEdge {
    /// The fallthrough edge, `to label %cont`.
    Default,
    /// Indirect destination `n`, the `n`th label in `[label %ind, ...]`.
    Indirect(usize),
}

/// A `ReshapeCfg` pass that narrows the entry block's `callbr` and redirects
/// `edge` onto `new_to`.
///
/// A refused redirect is recorded in `refusal` and the pass still finishes, so
/// the caller gets the module back and can compare it with the input: an
/// `Err` from `run` would consume the module and leave nothing to compare.
struct RedirectCallBrEdge<B: ModuleBrand> {
    edge: CallBrEdge,
    new_to: BlockId<Dyn, B>,
    refusal: Rc<RefCell<Option<IrError>>>,
}

impl<B: ModuleBrand> RedirectCallBrEdge<B> {
    fn new(edge: CallBrEdge, new_to: BlockId<Dyn, B>) -> Self {
        Self {
            edge,
            new_to,
            refusal: Rc::new(RefCell::new(None)),
        }
    }
}

impl<B: ModuleBrand> FunctionPass<B> for RedirectCallBrEdge<B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "redirect-callbr-edge";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport<B>>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let reshape = cx.mutate();
        let entry = reshape
            .function()
            .entry_block()
            .expect("definition has entry");
        let callbr = reshape.edit_callbr(entry.id())?;
        let redirected = match self.edge {
            CallBrEdge::Default => callbr.redirect_default(self.new_to, &[]),
            CallBrEdge::Indirect(index) => callbr.redirect_indirect(index, self.new_to, &[]),
        };
        if let Err(error) = redirected {
            *self.refusal.borrow_mut() = Some(error);
        }
        Ok(reshape.done())
    }
}

/// Build `void @caller()` with a `callbr void asm sideeffect "", "!i"() to
/// label %cont [label %ind]`, plus an unreferenced `%new` block a redirect can
/// aim at.
///
/// The callee is inline asm because `Verifier::visitCallBrInst`'s
/// non-inline-asm arm ends in `default: CheckFailed("Callbr currently only
/// supports asm-goto and selected intrinsics")` — a `callbr` to an ordinary
/// function is not IR upstream accepts, and these tests re-verify their output.
/// One `!i` label constraint matches the one indirect destination.
fn build_callbr_caller<'ctx, B: ModuleBrand + 'ctx>(
    key: &mut LayoutToken<'ctx, B>,
) -> IrResult<CallerFixture<B>> {
    let m = key.module();
    let void_ty = m.void_type();
    let asm_ty = m.function_type(void_ty.as_type(), Vec::<llvmkit_ir::Type<'_, _>>::new())?;
    let asm = m.inline_asm(
        asm_ty,
        "",
        "!i",
        llvmkit_ir::InlineAsmOptions::new().side_effects(),
    );
    let caller = m
        .add_typed_function::<(), (), _>("caller", Linkage::External)?
        .as_function();

    let entry = m.view(caller).append_basic_block(m, "entry");
    let cont = m.view(caller).append_basic_block(m, "cont");
    let ind = m.view(caller).append_basic_block(m, "ind");
    let new = m.view(caller).append_basic_block(m, "new");
    // Capture the labels before `position_at_end` consumes the block handles.
    let cont_lbl = cont.id();
    let ind_lbl = ind.id();
    let new_dyn: BasicBlockLabel<'_, Dyn, _> = new.to_erased().try_into()?;
    let new_dyn = new_dyn.id();

    let bc = IrBuilder::new_for::<()>(key).position_at_end(cont);
    bc.ret_void();
    let bi = IrBuilder::new_for::<()>(key).position_at_end(ind);
    bi.unreachable();
    let bnew = IrBuilder::new_for::<()>(key).position_at_end(new);
    bnew.ret_void();

    let b = IrBuilder::new_for::<()>(key).position_at_end(entry);
    let _ = b.inline_asm_callbr::<(), _, _, _, _, _, _>(
        asm,
        Vec::<Value<'_, _>>::new(),
        cont_lbl,
        [ind_lbl],
        "",
    )?;
    Ok((caller, new_dyn))
}

/// `edit_callbr(..).redirect_default(new, [])` retargets the fallthrough edge.
#[test]
fn callbr_redirect_default_retargets_default_edge() -> Result<(), IrError> {
    let mut module = module_new!("callbr-redirect-default")?;
    let mut key = module.layout_token();
    let (caller, new_dyn) = build_callbr_caller(&mut key)?;
    let verified = module.verify()?;
    let mut analyses = Analyses::new();
    let pass = RedirectCallBrEdge::new(CallBrEdge::Default, new_dyn);
    let refusal = Rc::clone(&pass.refusal);
    let out = run_function_pass(pass, verified, caller, &mut analyses)?;
    assert_eq!(
        *refusal.borrow(),
        None,
        "the default edge must be redirected"
    );
    let reverified = out.verify().expect("callbr redirect output must re-verify");
    let printed = format!("{reverified}");
    assert!(
        printed.contains("to label %new [label %ind]"),
        "default edge must now target %new, indirect untouched, got:\n{printed}"
    );
    Ok(())
}

/// `redirect_indirect` past the end of the `callbr`'s indirect destinations is
/// refused with the index and the count, and the function is left exactly as
/// it was: the refusal comes before any edge is touched.
///
/// **No upstream counterpart**: `CallBrInst::setIndirectDest` asserts its
/// index; llvmkit's typed refusal carries it.
#[test]
fn callbr_redirect_indirect_refuses_an_index_past_the_destinations() -> Result<(), IrError> {
    let mut module = module_new!("callbr-redirect-indirect-out-of-range")?;
    let mut key = module.layout_token();
    let m = key.module();
    let (caller, new_dyn) = build_callbr_caller(&mut key)?;
    let before = format!("{m}");
    let verified = module.verify()?;
    let mut analyses = Analyses::new();
    let pass = RedirectCallBrEdge::new(CallBrEdge::Indirect(1), new_dyn);
    let refusal = Rc::clone(&pass.refusal);
    let out = run_function_pass(pass, verified, caller, &mut analyses)?;
    let expected =
        IrError::from(CfgEditError::IndirectDestinationOutOfRange { index: 1, count: 1 });
    assert_eq!(
        *refusal.borrow(),
        Some(expected.clone()),
        "index 1 of a callbr with one indirect destination must be refused"
    );
    assert_eq!(
        expected.to_string(),
        "redirect_indirect: `callbr` indirect destination index 1 out of range for 1 indirect \
         destinations"
    );
    assert_eq!(
        format!("{out}"),
        before,
        "a refused redirect must leave the function unchanged"
    );
    Ok(())
}

/// `edit_callbr(..).redirect_indirect(0, new, [])` retargets indirect edge 0.
#[test]
fn callbr_redirect_indirect_retargets_indirect_edge() -> Result<(), IrError> {
    let mut module = module_new!("callbr-redirect-indirect")?;
    let mut key = module.layout_token();
    let (caller, new_dyn) = build_callbr_caller(&mut key)?;
    let verified = module.verify()?;
    let mut analyses = Analyses::new();
    let pass = RedirectCallBrEdge::new(CallBrEdge::Indirect(0), new_dyn);
    let refusal = Rc::clone(&pass.refusal);
    let out = run_function_pass(pass, verified, caller, &mut analyses)?;
    assert_eq!(
        *refusal.borrow(),
        None,
        "indirect edge 0 must be redirected"
    );
    let reverified = out.verify().expect("callbr redirect output must re-verify");
    let printed = format!("{reverified}");
    assert!(
        printed.contains("to label %cont [label %new]"),
        "indirect edge 0 must now target %new, default untouched, got:\n{printed}"
    );
    Ok(())
}

/// A `ReshapeCfg` pass that narrows `block`'s terminator with the `edit_*`
/// entry for `kind` and records the refusal.
struct NarrowAs<B: ModuleBrand> {
    kind: EditedTerminator,
    block: BlockId<Dyn, B>,
    refusal: Rc<RefCell<Option<IrError>>>,
}

impl<B: ModuleBrand> FunctionPass<B> for NarrowAs<B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "narrow-as";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport<B>>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let reshape = cx.mutate();
        let refused = match self.kind {
            EditedTerminator::Switch => reshape.edit_switch(self.block).err(),
            EditedTerminator::ConditionalBranch => reshape.edit_cond_br(self.block).err(),
            EditedTerminator::Branch => reshape.edit_br(self.block).err(),
            EditedTerminator::Invoke => reshape.edit_invoke(self.block).err(),
            EditedTerminator::CallBr => reshape.edit_callbr(self.block).err(),
        };
        *self.refusal.borrow_mut() = refused;
        Ok(reshape.done())
    }
}

/// Every typed narrowing refuses a block whose terminator is of another kind
/// — here `%new`'s `ret void` — with the kind it expected, and says so in its
/// text.
///
/// **No upstream counterpart**: upstream narrows with `cast<SwitchInst>` and
/// friends, which assert; the typed refusal and its text are llvmkit's.
#[test]
fn every_typed_narrowing_refuses_a_terminator_of_another_kind() -> Result<(), IrError> {
    for (kind, text) in [
        (
            EditedTerminator::Switch,
            "edit_switch: `from`'s terminator is not a `switch`",
        ),
        (
            EditedTerminator::ConditionalBranch,
            "edit_cond_br: `from`'s terminator is not a `cond_br`",
        ),
        (
            EditedTerminator::Branch,
            "edit_br: `from`'s terminator is not an unconditional `br`",
        ),
        (
            EditedTerminator::Invoke,
            "edit_invoke: `from`'s terminator is not an `invoke`",
        ),
        (
            EditedTerminator::CallBr,
            "edit_callbr: `from`'s terminator is not a `callbr`",
        ),
    ] {
        let mut module = module_new!("narrow-a-ret")?;
        let mut key = module.layout_token();
        let (caller, new_dyn) = build_callbr_caller(&mut key)?;
        let pass = NarrowAs {
            kind,
            block: new_dyn,
            refusal: Rc::new(RefCell::new(None)),
        };
        let refusal = Rc::clone(&pass.refusal);
        let refused = refusal_leaving_module_unchanged(module, caller, pass, &refusal)?;
        let expected = IrError::from(CfgEditError::TerminatorKindMismatch { expected: kind });
        assert_eq!(refused, Some(expected.clone()), "{kind:?}");
        assert_eq!(expected.to_string(), text, "{kind:?}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Function scope: a function pass names only its own function's blocks.

/// What a [`ReachOutsideFunction`] pass tries with a block that is not in the
/// function it runs on.
enum OutsideReach<B: ModuleBrand> {
    /// `edit_terminator` of the block.
    EditTerminator(BlockId<Dyn, B>),
    /// `builder_at` an insert point in the block. An `Option` because the
    /// restore consumes it and `run` takes `&mut self`.
    BuildAt(Option<InsertPoint<'static, (), B, PositionedBefore>>),
    /// `builder_before` the first instruction of `@other`, found by the pass
    /// through its module view.
    BuildBeforeOther,
}

/// A `ReshapeCfg` pass that reaches for a block outside its function and
/// records the refusal, finishing so the caller can compare the module.
struct ReachOutsideFunction<B: ModuleBrand> {
    reach: OutsideReach<B>,
    refusal: Rc<RefCell<Option<IrError>>>,
}

impl<B: ModuleBrand> ReachOutsideFunction<B> {
    fn new(reach: OutsideReach<B>) -> Self {
        Self {
            reach,
            refusal: Rc::new(RefCell::new(None)),
        }
    }
}

impl<B: ModuleBrand> FunctionPass<B> for ReachOutsideFunction<B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "reach-outside-function";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport<B>>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let mut reshape = cx.mutate();
        let refused = match &mut self.reach {
            OutsideReach::EditTerminator(block) => reshape.edit_terminator(*block).err(),
            OutsideReach::BuildAt(at) => reshape
                .builder_at(at.take().expect("the pass runs once"))
                .err(),
            OutsideReach::BuildBeforeOther => {
                let other = reshape
                    .module()
                    .functions()
                    .find(|function| function.name().as_deref() == Some("other"))
                    .expect("@other is in the module");
                let anchor = other
                    .entry_block()
                    .expect("@other has a body")
                    .placed_instructions()
                    .next()
                    .expect("@other's entry holds its `ret`");
                reshape.builder_before(anchor).err()
            }
        };
        *self.refusal.borrow_mut() = refused;
        Ok(reshape.done())
    }
}

/// The callbr fixture plus a second function, `void @other()`, whose `entry`
/// holds a lone `ret void`, and an orphan block that belongs to no function.
struct TwoFunctionFixture<B: ModuleBrand> {
    caller: FunctionId<(), B>,
    other_entry: BlockId<Dyn, B>,
    before_other_ret: InsertPoint<'static, (), B, PositionedBefore>,
    orphan: BlockId<Dyn, B>,
}

fn build_two_functions<'ctx, B: ModuleBrand + 'ctx>(
    key: &mut LayoutToken<'ctx, B>,
) -> IrResult<TwoFunctionFixture<B>> {
    let m = key.module();
    let (caller, _new) = build_callbr_caller(key)?;
    let other = m
        .add_typed_function::<(), (), _>("other", Linkage::External)?
        .as_function();
    let other_entry = m.view(other).append_basic_block(m, "entry");
    let other_entry_label: BasicBlockLabel<'_, Dyn, _> = other_entry.to_erased().try_into()?;
    let other_entry_id = other_entry_label.id();
    let (_block, ret) = IrBuilder::new_for::<()>(key)
        .position_at_end(other_entry)
        .ret_void();
    let before_other_ret = IrBuilder::new_for::<()>(key)
        .position_before(ret.placed())?
        .save_insert_point();
    let orphan = BasicBlock::create_orphan(m, "orphan").id();
    Ok(TwoFunctionFixture {
        caller,
        other_entry: other_entry_id,
        before_other_ret,
        orphan,
    })
}

/// Run `pass` over `@caller` and return its recorded refusal, asserting the
/// printed module is unchanged by it.
fn refusal_leaving_module_unchanged<B: ModuleBrand>(
    m: Module<B, llvmkit_ir::Unverified>,
    caller: FunctionId<(), B>,
    pass: impl FunctionPass<B, Access = ReshapeCfg, Requires = ()>,
    refusal: &Rc<RefCell<Option<IrError>>>,
) -> Result<Option<IrError>, IrError> {
    let before = format!("{m}");
    let verified = m.verify()?;
    let mut analyses = Analyses::new();
    let out = run_function_pass(pass, verified, caller, &mut analyses)?;
    assert_eq!(
        format!("{out}"),
        before,
        "a refused reach outside the function must leave the module unchanged"
    );
    Ok(refusal.borrow_mut().take())
}

/// `edit_terminator` of another function's block is refused before the edit
/// reads it: a function pass reshapes its own function and no other.
///
/// **No upstream counterpart**: the rule is upstream's —
/// `ModuleToFunctionPassAdaptor`'s comment (`include/llvm/IR/PassManager.h`)
/// says function passes "should not read or modify any other functions!" —
/// but nothing there checks it; llvmkit's typed refusal is its own.
#[test]
fn a_function_pass_cannot_edit_another_functions_terminator() -> Result<(), IrError> {
    let mut module = module_new!("edit-another-functions-terminator")?;
    let mut key = module.layout_token();
    let fixture = build_two_functions(&mut key)?;
    let pass = ReachOutsideFunction::new(OutsideReach::EditTerminator(fixture.other_entry));
    let refusal = Rc::clone(&pass.refusal);
    let refused = refusal_leaving_module_unchanged(module, fixture.caller, pass, &refusal)?;
    assert_eq!(
        refused,
        Some(IrError::from(llvmkit_ir::BlockError::BlockNotInFunction))
    );
    Ok(())
}

/// `edit_terminator` of an orphan block, one that belongs to no function, is
/// refused as not in the function, ahead of the empty block's missing
/// terminator — which is why `CfgEditError::NoTerminator` stays unreachable
/// from a pass.
///
/// **No upstream counterpart**: as above.
#[test]
fn a_function_pass_cannot_edit_an_orphan_blocks_terminator() -> Result<(), IrError> {
    let mut module = module_new!("edit-orphan-terminator")?;
    let mut key = module.layout_token();
    let fixture = build_two_functions(&mut key)?;
    let pass = ReachOutsideFunction::new(OutsideReach::EditTerminator(fixture.orphan));
    let refusal = Rc::clone(&pass.refusal);
    let refused = refusal_leaving_module_unchanged(module, fixture.caller, pass, &refusal)?;
    assert_eq!(
        refused,
        Some(IrError::from(llvmkit_ir::BlockError::BlockNotInFunction))
    );
    Ok(())
}

/// A redirect onto another function's block is refused before any edge or
/// phi is touched: a branch may name only a block of its own function.
///
/// **No upstream counterpart**: as above. Upstream's `Verifier` rejects the
/// resulting cross-function branch afterwards; llvmkit refuses the edit
/// itself.
#[test]
fn a_function_pass_cannot_redirect_onto_another_functions_block() -> Result<(), IrError> {
    let mut module = module_new!("redirect-onto-another-function")?;
    let mut key = module.layout_token();
    let fixture = build_two_functions(&mut key)?;
    let pass = RedirectCallBrEdge::new(CallBrEdge::Default, fixture.other_entry);
    let refusal = Rc::clone(&pass.refusal);
    let refused = refusal_leaving_module_unchanged(module, fixture.caller, pass, &refusal)?;
    assert_eq!(
        refused,
        Some(IrError::from(llvmkit_ir::BlockError::BlockNotInFunction))
    );
    Ok(())
}

/// `builder_at` an insert point in another function is refused, so the pass
/// gets no builder that could insert there.
///
/// **No upstream counterpart**: as above.
#[test]
fn a_function_pass_cannot_build_into_another_function() -> Result<(), IrError> {
    let mut module = module_new!("build-into-another-function")?;
    let mut key = module.layout_token();
    let fixture = build_two_functions(&mut key)?;
    let pass = ReachOutsideFunction::new(OutsideReach::BuildAt(Some(fixture.before_other_ret)));
    let refusal = Rc::clone(&pass.refusal);
    let refused = refusal_leaving_module_unchanged(module, fixture.caller, pass, &refusal)?;
    assert_eq!(
        refused,
        Some(IrError::from(llvmkit_ir::BlockError::BlockNotInFunction))
    );
    Ok(())
}

/// `builder_before` an instruction of another function is refused, so the
/// pass gets no builder there either.
///
/// **No upstream counterpart**: as above.
#[test]
fn a_function_pass_cannot_build_before_another_functions_instruction() -> Result<(), IrError> {
    let mut module = module_new!("build-before-another-function")?;
    let mut key = module.layout_token();
    let fixture = build_two_functions(&mut key)?;
    let pass = ReachOutsideFunction::new(OutsideReach::BuildBeforeOther);
    let refusal = Rc::clone(&pass.refusal);
    let refused = refusal_leaving_module_unchanged(module, fixture.caller, pass, &refusal)?;
    assert_eq!(
        refused,
        Some(IrError::from(llvmkit_ir::BlockError::BlockNotInFunction))
    );
    Ok(())
}

/// A `ReshapeCfg` pass that inserts `%slot = alloca i32` before its entry
/// block's terminator through `builder_before`.
struct AllocaBeforeEntryTerminator;

impl<B: ModuleBrand> FunctionPass<B> for AllocaBeforeEntryTerminator {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "alloca-before-entry-terminator";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport<B>>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let mut reshape = cx.mutate();
        let terminator = reshape
            .function()
            .entry_block()
            .expect("definition has entry")
            .placed_instructions()
            .last()
            .expect("the entry block ends in its terminator");
        let i32_ty = reshape.module().i32_type();
        reshape.builder_before(terminator)?.alloca(i32_ty, "slot")?;
        Ok(reshape.done())
    }
}

/// A function pass starts a builder at one of its own instructions with
/// `builder_before` — the port of constructing `IRBuilder<> B(I)` in a pass —
/// and what it builds lands right before that instruction.
///
/// **No upstream counterpart** as a test: llvmkit's pass context hands the
/// builder out, where an upstream pass constructs `IRBuilder` itself.
#[test]
fn a_function_pass_builds_before_its_own_instruction() -> Result<(), IrError> {
    let mut module = module_new!("build-before-own-instruction")?;
    let mut key = module.layout_token();
    let (caller, _) = build_callbr_caller(&mut key)?;
    let verified = module.verify()?;
    let mut analyses = Analyses::new();
    let out = run_function_pass(AllocaBeforeEntryTerminator, verified, caller, &mut analyses)?;
    let reverified = out.verify().expect("the inserted alloca re-verifies");
    let printed = format!("{reverified}");
    assert!(
        printed.contains("entry:\n  %slot = alloca i32, align 4\n  callbr void"),
        "the alloca lands right before the entry's `callbr`, got:\n{printed}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// cond_br — remove an arm via the typed handle (collapse to `br`).
// ---------------------------------------------------------------------------

/// A `ReshapeCfg` pass that narrows the entry block's `cond_br` and removes one
/// arm (chosen by `remove_then`), collapsing it to an unconditional `br`.
struct RemoveCondBrArm {
    remove_then: bool,
}

impl<B: ModuleBrand> FunctionPass<B> for RemoveCondBrArm {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "remove-condbr-arm";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport<B>>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let reshape = cx.mutate();
        let entry = reshape
            .function()
            .entry_block()
            .expect("definition has entry");
        let cond_br = reshape.edit_cond_br(entry.id())?;
        if self.remove_then {
            cond_br.remove_then()?;
        } else {
            cond_br.remove_else()?;
        }
        Ok(reshape.done())
    }
}

/// Build `i32 @f(i32 %a)` whose entry is `cond_br (%a == 0) ? then : else`.
fn build_cond_br_fn<'ctx, B: ModuleBrand + 'ctx>(
    key: &mut LayoutToken<'ctx, B>,
) -> IrResult<FunctionId<i32, B>> {
    let m = key.module();
    let i32_ty = m.i32_type();
    let f = m
        .add_typed_function::<i32, (i32,), _>("f", Linkage::External)?
        .as_function();
    let entry = m.view(f).append_basic_block(m, "entry");
    let then_bb = m.view(f).append_basic_block(m, "then");
    let else_bb = m.view(f).append_basic_block(m, "else");
    // Capture the labels before `position_at_end` consumes the block handles.
    let then_lbl = then_bb.id();
    let else_lbl = else_bb.id();

    let bt = IrBuilder::new_for::<i32>(key).position_at_end(then_bb);
    bt.ret(i32_ty.const_int(0_u32))?;
    let be = IrBuilder::new_for::<i32>(key).position_at_end(else_bb);
    be.ret(i32_ty.const_int(1_u32))?;

    let b = IrBuilder::new_for::<i32>(key).position_at_end(entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let c = b.int_cmp::<i32, _, _, _>(IntPredicate::Eq, a, 0_i32, "c")?;
    b.cond_br(c, then_lbl, else_lbl)?;
    Ok(f)
}

/// `edit_cond_br(..).remove_then()` collapses the `cond_br` to `br label
/// %else` (the survivor), and the output re-verifies.
#[test]
fn cond_br_remove_then_collapses_to_br() -> Result<(), IrError> {
    let mut module = module_new!("condbr-remove-then")?;
    let mut key = module.layout_token();
    let f = build_cond_br_fn(&mut key)?;
    let verified = module.verify()?;
    let mut analyses = Analyses::new();
    let pass = RemoveCondBrArm { remove_then: true };
    let out = run_function_pass(pass, verified, f, &mut analyses)?;
    let reverified = out.verify().expect("cond_br collapse must re-verify");
    let printed = format!("{reverified}");
    assert!(
        printed.contains("br label %else"),
        "removing the then arm must leave `br label %else`, got:\n{printed}"
    );
    assert!(
        !printed.contains("br i1 %c"),
        "the cond_br must be gone (no conditional branch left), got:\n{printed}"
    );
    Ok(())
}

/// `edit_cond_br(..).remove_else()` collapses the `cond_br` to `br label
/// %then` (the survivor).
#[test]
fn cond_br_remove_else_collapses_to_br() -> Result<(), IrError> {
    let mut module = module_new!("condbr-remove-else")?;
    let mut key = module.layout_token();
    let f = build_cond_br_fn(&mut key)?;
    let verified = module.verify()?;
    let mut analyses = Analyses::new();
    let pass = RemoveCondBrArm { remove_then: false };
    let out = run_function_pass(pass, verified, f, &mut analyses)?;
    let reverified = out.verify().expect("cond_br collapse must re-verify");
    let printed = format!("{reverified}");
    assert!(
        printed.contains("br label %then"),
        "removing the else arm must leave `br label %then`, got:\n{printed}"
    );
    assert!(
        !printed.contains("br i1 %c"),
        "the cond_br must be gone (no conditional branch left), got:\n{printed}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// switch — redirect / remove a case via the typed handle.
// ---------------------------------------------------------------------------

/// A `ReshapeCfg` pass that narrows the entry block's `switch` and either
/// redirects case `case0` onto `new_to` or removes it.
struct SwitchCaseOp<B: ModuleBrand> {
    remove: bool,
    case0: BlockId<Dyn, B>,
    new_to: BlockId<Dyn, B>,
}

impl<B: ModuleBrand> FunctionPass<B> for SwitchCaseOp<B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "switch-case-op";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport<B>>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let reshape = cx.mutate();
        let entry = reshape
            .function()
            .entry_block()
            .expect("definition has entry");
        let switch = reshape.edit_switch(entry.id())?;
        if self.remove {
            switch.remove_successor(self.case0)?;
        } else {
            switch.redirect_successor(self.case0, self.new_to, &[])?;
        }
        Ok(reshape.done())
    }
}

/// Build `i32 @f(i32 %a)` whose entry is `switch %a, default %dflt [ 0 ->
/// case0, 1 -> case1 ]`, plus an unreferenced `%new` block. Returns the
/// function and the `case0`/`new` `Dyn` labels.
fn build_switch_fn<'ctx, B: ModuleBrand + 'ctx>(
    key: &mut LayoutToken<'ctx, B>,
) -> IrResult<SwitchFixture<B>> {
    let m = key.module();
    let i32_ty = m.i32_type();
    let f = m
        .add_typed_function::<i32, (i32,), _>("f", Linkage::External)?
        .as_function();
    let entry = m.view(f).append_basic_block(m, "entry");
    let dflt = m.view(f).append_basic_block(m, "dflt");
    let case0 = m.view(f).append_basic_block(m, "case0");
    let case1 = m.view(f).append_basic_block(m, "case1");
    let new = m.view(f).append_basic_block(m, "new");
    // Capture the labels before `position_at_end` consumes the block handles.
    let dflt_lbl = dflt.id();
    let case0_lbl = case0.id();
    let case1_lbl = case1.id();
    let case0_dyn: BasicBlockLabel<'_, Dyn, _> = case0.to_erased().try_into()?;
    let case0_dyn = case0_dyn.id();
    let new_dyn: BasicBlockLabel<'_, Dyn, _> = new.to_erased().try_into()?;
    let new_dyn = new_dyn.id();

    for (bb, k) in [(dflt, 0_u32), (case0, 1), (case1, 2), (new, 3)] {
        let bb_b = IrBuilder::new_for::<i32>(key).position_at_end(bb);
        bb_b.ret(i32_ty.const_int(k))?;
    }

    let b = IrBuilder::new_for::<i32>(key).position_at_end(entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let (_sealed, sw) = b.switch_dyn(a, dflt_lbl, "")?;
    let sw = sw.add_case(i32_ty.const_int(0_u32), case0_lbl)?;
    sw.add_case(i32_ty.const_int(1_u32), case1_lbl)?.finish();
    Ok((f, case0_dyn, new_dyn))
}

/// `edit_switch(..).redirect_successor(case0, new, [])` retargets the case-0
/// edge onto `%new`, and the output re-verifies.
#[test]
fn switch_redirect_successor_retargets_case() -> Result<(), IrError> {
    let mut module = module_new!("switch-redirect-succ")?;
    let mut key = module.layout_token();
    let (f, case0_dyn, new_dyn) = build_switch_fn(&mut key)?;
    let verified = module.verify()?;
    let mut analyses = Analyses::new();
    let pass = SwitchCaseOp {
        remove: false,
        case0: case0_dyn,
        new_to: new_dyn,
    };
    let out = run_function_pass(pass, verified, f, &mut analyses)?;
    let reverified = out.verify().expect("switch redirect must re-verify");
    let printed = format!("{reverified}");
    assert!(
        printed.contains("i32 0, label %new"),
        "case 0 must now target %new, got:\n{printed}"
    );
    assert!(
        !printed.contains("i32 0, label %case0"),
        "case 0 must no longer target %case0, got:\n{printed}"
    );
    Ok(())
}

/// `edit_switch(..).remove_successor(case0)` drops the case-0 edge; case 1 and
/// the default survive, and the output re-verifies.
#[test]
fn switch_remove_successor_drops_case() -> Result<(), IrError> {
    let mut module = module_new!("switch-remove-succ")?;
    let mut key = module.layout_token();
    let (f, case0_dyn, new_dyn) = build_switch_fn(&mut key)?;
    let verified = module.verify()?;
    let mut analyses = Analyses::new();
    let pass = SwitchCaseOp {
        remove: true,
        case0: case0_dyn,
        new_to: new_dyn,
    };
    let out = run_function_pass(pass, verified, f, &mut analyses)?;
    let reverified = out.verify().expect("switch remove must re-verify");
    let printed = format!("{reverified}");
    assert!(
        !printed.contains("i32 0, label %case0"),
        "the case-0 edge must be gone, got:\n{printed}"
    );
    assert!(
        printed.contains("i32 1, label %case1"),
        "case 1 must survive, got:\n{printed}"
    );
    assert!(
        printed.contains("label %dflt ["),
        "the default must survive, got:\n{printed}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// switch — reject a target-based op whose `old_to` is not a live case.
// ---------------------------------------------------------------------------

/// A `ReshapeCfg` pass that narrows the entry block's `switch` and calls
/// `redirect_successor(old_to, new_to, phi_values)`. Unlike `SwitchCaseOp`, it
/// carries an explicit `phi_values` so the bogus-target test can seed a
/// phi-arity-valid redirect (making the pre-fix path sail past already-reaches
/// + phi validation before it would corrupt `new_to`'s phi).
struct RedirectSwitchSuccessor<B: ModuleBrand> {
    old_to: BlockId<Dyn, B>,
    new_to: BlockId<Dyn, B>,
    phi_values: Vec<ValueId<B>>,
}

impl<B: ModuleBrand> FunctionPass<B> for RedirectSwitchSuccessor<B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "redirect-switch-successor";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport<B>>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let reshape = cx.mutate();
        let entry = reshape
            .function()
            .entry_block()
            .expect("definition has entry");
        let switch = reshape.edit_switch(entry.id())?;
        switch.redirect_successor(self.old_to, self.new_to, &self.phi_values)?;
        Ok(reshape.done())
    }
}

/// Build `i32 @f(i32 %a)`:
/// ```text
/// entry:    %ev = add %a, 3 ; switch %a, default %dflt [ 0 -> case0 ]
/// dflt:     %nd = add %a, 5 ; br new(%nd)
/// case0:    ret 0
/// bogus:    ret 99            ; a real block, but NOT a case target
/// new(%np): ret %np           ; head-phi seeded by `dflt`
/// ```
/// `bogus` is neither a case destination nor the default, so a target-based op
/// naming it must be rejected. `new` carries a head-phi (arity 1) so a pre-fix
/// `redirect_successor(bogus, new, [ev])` would pass phi validation and corrupt
/// `new`'s phi; `%ev` (defined in `entry`) is a valid seed for it.
fn build_switch_bogus_fn<'ctx, B: ModuleBrand + 'ctx>(
    key: &mut LayoutToken<'ctx, B>,
) -> IrResult<SwitchBogusFixture<B>> {
    let m = key.module();
    let i32_ty = m.i32_type();
    let f = m
        .add_typed_function::<i32, (i32,), _>("f", Linkage::External)?
        .as_function();
    let entry = m.view(f).append_basic_block(m, "entry");
    let dflt = m.view(f).append_basic_block(m, "dflt");
    let case0 = m.view(f).append_basic_block(m, "case0");
    let bogus = m.view(f).append_basic_block(m, "bogus");
    // new(%np: i32): a head-phi authored as a block parameter, seeded by `dflt`.
    let (new, new_params) = IrBuilder::new_for::<i32>(key).append_block_with_params(
        m.view(f),
        &[i32_ty.as_type()],
        "new",
    )?;

    let dflt_lbl = dflt.id();
    let case0_lbl = case0.id();
    let new_lbl = new.id();
    let bogus_dyn: BasicBlockLabel<'_, Dyn, _> = bogus.to_erased().try_into()?;
    let bogus_dyn = bogus_dyn.id();
    let new_dyn: BasicBlockLabel<'_, Dyn, _> = new.to_erased().try_into()?;
    let new_dyn = new_dyn.id();

    // entry: %ev = add %a, 3 ; switch %a, default %dflt [ 0 -> case0 ]
    let b = IrBuilder::new_for::<i32>(key).position_at_end(entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let ev = b.int_add(a, 3_i32, "ev")?;
    let (_sealed, sw) = b.switch_dyn(a, dflt_lbl, "")?;
    sw.add_case(i32_ty.const_int(0_u32), case0_lbl)?.finish();

    // dflt: %nd = add %a, 5 ; br new(%nd)
    let b = IrBuilder::new_for::<i32>(key).position_at_end(dflt);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let nd = b.int_add(a, 5_i32, "nd")?;
    b.br_with_args(new_lbl, &[m.view(nd).as_erased()])?;

    // case0: ret 0
    let b = IrBuilder::new_for::<i32>(key).position_at_end(case0);
    b.ret(i32_ty.const_int(0_u32))?;

    // bogus: ret 99 (a real block, but not reached from the switch's cases).
    let b = IrBuilder::new_for::<i32>(key).position_at_end(bogus);
    b.ret(i32_ty.const_int(99_u32))?;

    // new: ret %np (the head-phi param carrying dflt's branch argument).
    let b = IrBuilder::new_for::<i32>(key).position_at_end(new);
    let np: IntValue<'_, i32, _> = new_params[0].try_into()?;
    b.ret(np)?;

    Ok((f, bogus_dyn, new_dyn, m.view(ev).as_erased().id()))
}

/// `redirect_successor` rejects an `old_to` that is not a case successor of the
/// switch — even when `new_to`'s head-phi arity matches the seed, so the pre-fix
/// path would sail past already-reaches + phi validation, retarget zero cases,
/// and corrupt `new`'s phi (an incoming from a non-predecessor). Regression for
/// the missing target-liveness check.
#[test]
fn switch_redirect_successor_rejects_non_case_target() -> Result<(), IrError> {
    let mut module = module_new!("switch-redirect-bogus")?;
    let mut key = module.layout_token();
    let (f, bogus_dyn, new_dyn, ev) = build_switch_bogus_fn(&mut key)?;
    let verified = module.verify()?;
    let mut analyses = Analyses::new();
    let pass = RedirectSwitchSuccessor {
        old_to: bogus_dyn,
        new_to: new_dyn,
        phi_values: vec![ev],
    };
    let err = run_function_pass(pass, verified, f, &mut analyses)
        .expect_err("a non-case `old_to` must be rejected");
    assert_eq!(
        err,
        IrError::from(llvmkit_ir::CfgEditError::NotACaseSuccessor {
            edit: llvmkit_ir::SwitchSuccessorEdit::Redirect,
        })
    );
    Ok(())
}

/// `remove_successor` rejects an `old_to` that is not a case successor (and is
/// not the default) rather than dropping zero cases yet logging a spurious
/// `CfgUpdate::delete` for an edge that never existed. Regression for the
/// missing target-liveness check.
#[test]
fn switch_remove_successor_rejects_non_case_target() -> Result<(), IrError> {
    let mut module = module_new!("switch-remove-bogus")?;
    let mut key = module.layout_token();
    let (f, bogus_dyn, new_dyn, _ev) = build_switch_bogus_fn(&mut key)?;
    let verified = module.verify()?;
    let mut analyses = Analyses::new();
    let pass = SwitchCaseOp {
        remove: true,
        case0: bogus_dyn,
        new_to: new_dyn,
    };
    let err = run_function_pass(pass, verified, f, &mut analyses)
        .expect_err("a non-case `old_to` must be rejected");
    assert_eq!(
        err,
        IrError::from(llvmkit_ir::CfgEditError::NotACaseSuccessor {
            edit: llvmkit_ir::SwitchSuccessorEdit::Remove,
        })
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// TermEdit::Uneditable — a `ret` block narrows to the no-edit variant.
// ---------------------------------------------------------------------------

/// A `ReshapeCfg` pass that asserts `edit_terminator` on a `ret`-terminated
/// entry returns [`TermEdit::Uneditable`].
struct AssertUneditable;

impl<B: ModuleBrand> FunctionPass<B> for AssertUneditable {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "assert-uneditable";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport<B>>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let reshape = cx.mutate();
        let entry = reshape
            .function()
            .entry_block()
            .expect("definition has entry");
        let edit = reshape.edit_terminator(entry.id())?;
        assert!(
            matches!(edit, TermEdit::Uneditable(_)),
            "a `ret` terminator must narrow to TermEdit::Uneditable"
        );
        Ok(reshape.done())
    }
}

/// `edit_terminator` on a `ret`-terminated block yields `TermEdit::Uneditable`.
#[test]
fn edit_terminator_ret_is_uneditable() -> Result<(), IrError> {
    let mut module = module_new!("uneditable-ret")?;
    let mut key = module.layout_token();
    let m = key.module();
    let i32_ty = m.i32_type();
    let f = m
        .add_typed_function::<i32, (), _>("f", Linkage::External)?
        .as_function();
    let entry = m.view(f).append_basic_block(m, "entry");
    let b = IrBuilder::new_for::<i32>(&mut key).position_at_end(entry);
    b.ret(i32_ty.const_int(0_u32))?;

    let verified = module.verify()?;
    let mut analyses = Analyses::new();
    // The pass's internal assertion is the test; a clean run means it held.
    let _ = run_function_pass(AssertUneditable, verified, f, &mut analyses)?;
    Ok(())
}
