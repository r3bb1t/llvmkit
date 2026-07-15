//! Typed terminator edit-handle surface (`FnReshape::edit_*`).
//!
//! These tests exercise the typed edit handles introduced alongside the
//! invoke/callbr edge redirects: narrowing a terminator into a handle whose
//! *type* fixes which edge ops exist, then driving each op and re-verifying the
//! emitted IR. The compile-time half of the guarantee — that a handle simply
//! has no `remove_*` where an edge is not removable — lives in the
//! `tests/compile_fail/` fixtures; here we assert the *runtime* behavior of the
//! ops that do exist.

use llvmkit_ir::{
    Analyses, BasicBlockLabel, Dyn, FnCx, FnReport, FunctionPass, FunctionValue, IRBuilder,
    IntPredicate, IntValue, IrError, IrResult, Linkage, Module, ModuleBrand, ReshapeCfg, TermEdit,
    Type, Value, run_function_pass,
};

// ---------------------------------------------------------------------------
// invoke — NEW capability: redirect the normal / unwind edge.
// ---------------------------------------------------------------------------

/// A `ReshapeCfg` pass that narrows the entry block's `invoke` and redirects
/// one of its two edges (chosen by `which`) onto `new_to`.
struct RedirectInvokeEdge<'ctx, B: ModuleBrand + 'ctx> {
    which: InvokeArm,
    new_to: BasicBlockLabel<'ctx, Dyn, B>,
}

#[derive(Clone, Copy)]
enum InvokeArm {
    Normal,
    Unwind,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for RedirectInvokeEdge<'ctx, B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "redirect-invoke-edge";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let entry = reshape
            .function()
            .entry_block()
            .expect("definition has entry");
        let invoke = reshape.edit_invoke(&entry)?;
        match self.which {
            InvokeArm::Normal => invoke.redirect_normal(&self.new_to, &[])?,
            InvokeArm::Unwind => invoke.redirect_unwind(&self.new_to, &[])?,
        }
        Ok(reshape.done())
    }
}

/// Build `void @caller()` with an `invoke void @callee() to label %normal
/// unwind label %unwind`, plus an unreferenced `%new` block that a redirect can
/// aim at. Returns the caller and the `%new` `Dyn` label.
#[allow(clippy::type_complexity)]
fn build_invoke_caller<'ctx>(
    m: &Module<'ctx, llvmkit_ir::Brand<'ctx>, llvmkit_ir::Unverified>,
) -> IrResult<(FunctionValue<'ctx, ()>, BasicBlockLabel<'ctx, Dyn>)> {
    let void_ty = m.void_type();
    let callee_ty = m.fn_type(void_ty.as_type(), Vec::<Type>::new(), false);
    let callee = m.add_function::<(), _>("callee", callee_ty, Linkage::External)?;
    let caller_ty = m.fn_type(void_ty.as_type(), Vec::<Type>::new(), false);
    let caller = m.add_function::<(), _>("caller", caller_ty, Linkage::External)?;

    let entry = caller.append_basic_block(m, "entry");
    let normal = caller.append_basic_block(m, "normal");
    let unwind = caller.append_basic_block(m, "unwind");
    let new = caller.append_basic_block(m, "new");
    // Capture the labels before `position_at_end` consumes the block handles.
    let normal_lbl = normal.label();
    let unwind_lbl = unwind.label();
    let new_dyn: BasicBlockLabel<Dyn> = new.label().as_value().try_into()?;

    let bn = IRBuilder::new_for::<()>(m).position_at_end(normal);
    bn.build_ret_void();
    let bu = IRBuilder::new_for::<()>(m).position_at_end(unwind);
    bu.build_ret_void();
    let bnew = IRBuilder::new_for::<()>(m).position_at_end(new);
    bnew.build_ret_void();

    let b = IRBuilder::new_for::<()>(m).position_at_end(entry);
    let _ = b.build_invoke_dyn(callee, Vec::<Value>::new(), normal_lbl, unwind_lbl, "")?;
    Ok((caller, new_dyn))
}

/// `edit_invoke(..).redirect_normal(new, [])` retargets ONLY the normal edge;
/// the unwind edge is untouched, and the output re-verifies.
#[test]
fn invoke_redirect_normal_retargets_normal_edge() -> Result<(), IrError> {
    Module::with_new("invoke-redirect-normal", |m| {
        let (caller, new_dyn) = build_invoke_caller(&m)?;
        let verified = m.verify()?;
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
    })
}

/// `edit_invoke(..).redirect_unwind(new, [])` retargets ONLY the unwind edge.
#[test]
fn invoke_redirect_unwind_retargets_unwind_edge() -> Result<(), IrError> {
    Module::with_new("invoke-redirect-unwind", |m| {
        let (caller, new_dyn) = build_invoke_caller(&m)?;
        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let pass = RedirectInvokeEdge {
            which: InvokeArm::Unwind,
            new_to: new_dyn,
        };
        let out = run_function_pass(pass, verified, caller, &mut analyses)?;
        let reverified = out.verify().expect("invoke redirect output must re-verify");
        let printed = format!("{reverified}");
        assert!(
            printed.contains("to label %normal unwind label %new"),
            "unwind edge must now target %new, normal untouched, got:\n{printed}"
        );
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// callbr — redirect the default / an indirect edge.
// ---------------------------------------------------------------------------

/// A `ReshapeCfg` pass that narrows the entry block's `callbr` and redirects
/// either its default edge or its indirect edge 0 onto `new_to`.
struct RedirectCallBrEdge<'ctx, B: ModuleBrand + 'ctx> {
    default_edge: bool,
    new_to: BasicBlockLabel<'ctx, Dyn, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for RedirectCallBrEdge<'ctx, B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "redirect-callbr-edge";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let entry = reshape
            .function()
            .entry_block()
            .expect("definition has entry");
        let callbr = reshape.edit_callbr(&entry)?;
        if self.default_edge {
            callbr.redirect_default(&self.new_to, &[])?;
        } else {
            callbr.redirect_indirect(0, &self.new_to, &[])?;
        }
        Ok(reshape.done())
    }
}

/// Build `void @caller()` with a `callbr void @callee() to label %cont
/// [label %ind]`, plus an unreferenced `%new` block a redirect can aim at.
#[allow(clippy::type_complexity)]
fn build_callbr_caller<'ctx>(
    m: &Module<'ctx, llvmkit_ir::Brand<'ctx>, llvmkit_ir::Unverified>,
) -> IrResult<(FunctionValue<'ctx, ()>, BasicBlockLabel<'ctx, Dyn>)> {
    let void_ty = m.void_type();
    let callee_ty = m.fn_type(void_ty.as_type(), Vec::<Type>::new(), false);
    let callee = m.add_function::<(), _>("callee", callee_ty, Linkage::External)?;
    let caller_ty = m.fn_type(void_ty.as_type(), Vec::<Type>::new(), false);
    let caller = m.add_function::<(), _>("caller", caller_ty, Linkage::External)?;

    let entry = caller.append_basic_block(m, "entry");
    let cont = caller.append_basic_block(m, "cont");
    let ind = caller.append_basic_block(m, "ind");
    let new = caller.append_basic_block(m, "new");
    // Capture the labels before `position_at_end` consumes the block handles.
    let cont_lbl = cont.label();
    let ind_lbl = ind.label();
    let new_dyn: BasicBlockLabel<Dyn> = new.label().as_value().try_into()?;

    let bc = IRBuilder::new_for::<()>(m).position_at_end(cont);
    bc.build_ret_void();
    let bi = IRBuilder::new_for::<()>(m).position_at_end(ind);
    bi.build_unreachable();
    let bnew = IRBuilder::new_for::<()>(m).position_at_end(new);
    bnew.build_ret_void();

    let b = IRBuilder::new_for::<()>(m).position_at_end(entry);
    let _ = b.build_callbr(callee, Vec::<Value>::new(), cont_lbl, [ind_lbl], "")?;
    Ok((caller, new_dyn))
}

/// `edit_callbr(..).redirect_default(new, [])` retargets the fallthrough edge.
#[test]
fn callbr_redirect_default_retargets_default_edge() -> Result<(), IrError> {
    Module::with_new("callbr-redirect-default", |m| {
        let (caller, new_dyn) = build_callbr_caller(&m)?;
        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let pass = RedirectCallBrEdge {
            default_edge: true,
            new_to: new_dyn,
        };
        let out = run_function_pass(pass, verified, caller, &mut analyses)?;
        let reverified = out.verify().expect("callbr redirect output must re-verify");
        let printed = format!("{reverified}");
        assert!(
            printed.contains("to label %new [label %ind]"),
            "default edge must now target %new, indirect untouched, got:\n{printed}"
        );
        Ok(())
    })
}

/// `edit_callbr(..).redirect_indirect(0, new, [])` retargets indirect edge 0.
#[test]
fn callbr_redirect_indirect_retargets_indirect_edge() -> Result<(), IrError> {
    Module::with_new("callbr-redirect-indirect", |m| {
        let (caller, new_dyn) = build_callbr_caller(&m)?;
        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let pass = RedirectCallBrEdge {
            default_edge: false,
            new_to: new_dyn,
        };
        let out = run_function_pass(pass, verified, caller, &mut analyses)?;
        let reverified = out.verify().expect("callbr redirect output must re-verify");
        let printed = format!("{reverified}");
        assert!(
            printed.contains("to label %cont [label %new]"),
            "indirect edge 0 must now target %new, default untouched, got:\n{printed}"
        );
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// cond_br — remove an arm via the typed handle (collapse to `br`).
// ---------------------------------------------------------------------------

/// A `ReshapeCfg` pass that narrows the entry block's `cond_br` and removes one
/// arm (chosen by `remove_then`), collapsing it to an unconditional `br`.
struct RemoveCondBrArm {
    remove_then: bool,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for RemoveCondBrArm {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "remove-condbr-arm";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let entry = reshape
            .function()
            .entry_block()
            .expect("definition has entry");
        let cond_br = reshape.edit_cond_br(&entry)?;
        if self.remove_then {
            cond_br.remove_then()?;
        } else {
            cond_br.remove_else()?;
        }
        Ok(reshape.done())
    }
}

/// Build `i32 @f(i32 %a)` whose entry is `cond_br (%a == 0) ? then : else`.
fn build_cond_br_fn<'ctx>(
    m: &Module<'ctx, llvmkit_ir::Brand<'ctx>, llvmkit_ir::Unverified>,
) -> IrResult<FunctionValue<'ctx, i32>> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block(m, "entry");
    let then_bb = f.append_basic_block(m, "then");
    let else_bb = f.append_basic_block(m, "else");
    // Capture the labels before `position_at_end` consumes the block handles.
    let then_lbl = then_bb.label();
    let else_lbl = else_bb.label();

    let bt = IRBuilder::new_for::<i32>(m).position_at_end(then_bb);
    bt.build_ret(i32_ty.const_int(0_u32))?;
    let be = IRBuilder::new_for::<i32>(m).position_at_end(else_bb);
    be.build_ret(i32_ty.const_int(1_u32))?;

    let b = IRBuilder::new_for::<i32>(m).position_at_end(entry);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let c = b.build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, a, 0_i32, "c")?;
    b.build_cond_br(c, then_lbl, else_lbl)?;
    Ok(f)
}

/// `edit_cond_br(..).remove_then()` collapses the `cond_br` to `br label
/// %else` (the survivor), and the output re-verifies.
#[test]
fn cond_br_remove_then_collapses_to_br() -> Result<(), IrError> {
    Module::with_new("condbr-remove-then", |m| {
        let f = build_cond_br_fn(&m)?;
        let verified = m.verify()?;
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
    })
}

/// `edit_cond_br(..).remove_else()` collapses the `cond_br` to `br label
/// %then` (the survivor).
#[test]
fn cond_br_remove_else_collapses_to_br() -> Result<(), IrError> {
    Module::with_new("condbr-remove-else", |m| {
        let f = build_cond_br_fn(&m)?;
        let verified = m.verify()?;
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
    })
}

// ---------------------------------------------------------------------------
// switch — redirect / remove a case via the typed handle.
// ---------------------------------------------------------------------------

/// A `ReshapeCfg` pass that narrows the entry block's `switch` and either
/// redirects case `case0` onto `new_to` or removes it.
struct SwitchCaseOp<'ctx, B: ModuleBrand + 'ctx> {
    remove: bool,
    case0: BasicBlockLabel<'ctx, Dyn, B>,
    new_to: BasicBlockLabel<'ctx, Dyn, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for SwitchCaseOp<'ctx, B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "switch-case-op";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let entry = reshape
            .function()
            .entry_block()
            .expect("definition has entry");
        let switch = reshape.edit_switch(&entry)?;
        if self.remove {
            switch.remove_successor(&self.case0)?;
        } else {
            switch.redirect_successor(&self.case0, &self.new_to, &[])?;
        }
        Ok(reshape.done())
    }
}

/// Build `i32 @f(i32 %a)` whose entry is `switch %a, default %dflt [ 0 ->
/// case0, 1 -> case1 ]`, plus an unreferenced `%new` block. Returns the
/// function and the `case0`/`new` `Dyn` labels.
#[allow(clippy::type_complexity)]
fn build_switch_fn<'ctx>(
    m: &Module<'ctx, llvmkit_ir::Brand<'ctx>, llvmkit_ir::Unverified>,
) -> IrResult<(
    FunctionValue<'ctx, i32>,
    BasicBlockLabel<'ctx, Dyn>,
    BasicBlockLabel<'ctx, Dyn>,
)> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block(m, "entry");
    let dflt = f.append_basic_block(m, "dflt");
    let case0 = f.append_basic_block(m, "case0");
    let case1 = f.append_basic_block(m, "case1");
    let new = f.append_basic_block(m, "new");
    // Capture the labels before `position_at_end` consumes the block handles.
    let dflt_lbl = dflt.label();
    let case0_lbl = case0.label();
    let case1_lbl = case1.label();
    let case0_dyn: BasicBlockLabel<Dyn> = case0.label().as_value().try_into()?;
    let new_dyn: BasicBlockLabel<Dyn> = new.label().as_value().try_into()?;

    for (bb, k) in [(dflt, 0_u32), (case0, 1), (case1, 2), (new, 3)] {
        let bb_b = IRBuilder::new_for::<i32>(m).position_at_end(bb);
        bb_b.build_ret(i32_ty.const_int(k))?;
    }

    let b = IRBuilder::new_for::<i32>(m).position_at_end(entry);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let (_sealed, sw) = b.build_switch(a, dflt_lbl, "")?;
    let sw = sw.add_case(i32_ty.const_int(0_u32), case0_lbl)?;
    sw.add_case(i32_ty.const_int(1_u32), case1_lbl)?.finish();
    Ok((f, case0_dyn, new_dyn))
}

/// `edit_switch(..).redirect_successor(case0, new, [])` retargets the case-0
/// edge onto `%new`, and the output re-verifies.
#[test]
fn switch_redirect_successor_retargets_case() -> Result<(), IrError> {
    Module::with_new("switch-redirect-succ", |m| {
        let (f, case0_dyn, new_dyn) = build_switch_fn(&m)?;
        let verified = m.verify()?;
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
    })
}

/// `edit_switch(..).remove_successor(case0)` drops the case-0 edge; case 1 and
/// the default survive, and the output re-verifies.
#[test]
fn switch_remove_successor_drops_case() -> Result<(), IrError> {
    Module::with_new("switch-remove-succ", |m| {
        let (f, case0_dyn, new_dyn) = build_switch_fn(&m)?;
        let verified = m.verify()?;
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
    })
}

// ---------------------------------------------------------------------------
// TermEdit::Uneditable — a `ret` block narrows to the no-edit variant.
// ---------------------------------------------------------------------------

/// A `ReshapeCfg` pass that asserts `edit_terminator` on a `ret`-terminated
/// entry returns [`TermEdit::Uneditable`].
struct AssertUneditable;

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for AssertUneditable {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "assert-uneditable";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let entry = reshape
            .function()
            .entry_block()
            .expect("definition has entry");
        let edit = reshape.edit_terminator(&entry)?;
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
    Module::with_new("uneditable-ret", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        b.build_ret(i32_ty.const_int(0_u32))?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        // The pass's internal assertion is the test; a clean run means it held.
        let _ = run_function_pass(AssertUneditable, verified, f, &mut analyses)?;
        Ok(())
    })
}
