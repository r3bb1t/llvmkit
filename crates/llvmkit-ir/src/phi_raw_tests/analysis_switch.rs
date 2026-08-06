//! Raw-phi edge-op coverage relocated from `tests/analysis_preservation.rs`.
//!
//! These typed switch edge-edit cases (`edit_switch(..).remove_successor` /
//! `redirect_successor`) build a `switch`-fed
//! merge whose phi collects an incoming from a `switch` case edge — which the
//! block-argument authoring surface (`append_block_with_params` +
//! `*_with_args`) physically cannot express — so they keep the raw
//! `int_phi`/`add_incoming` API and run from inside the crate. They are
//! kept verbatim from their integration-test origin (only the `llvmkit_ir::`
//! paths are rewritten to `crate::`).

use crate::{
    Analyses, BlockId, Dyn, FnCx, FnReport, FunctionPass, IntValue, IrBuilder, IrError, IrResult,
    Linkage, Module, ModuleBrand, ReshapeCfg, ValueId, run_function_pass,
};

/// A `ReshapeCfg` pass that calls `edit_switch(from.id())?.remove_successor(to)` on
/// the block named `from_name`, dropping its edge to `to`. The `to` label is
/// stashed at build time (arena ids are stable across `verify()`), mirroring how
/// `InsertMergePhi` stashes its incomings.
struct RemoveSwitchEdge<B: ModuleBrand> {
    from_name: &'static str,
    to: BlockId<Dyn, B>,
}

impl<B: ModuleBrand> FunctionPass<B> for RemoveSwitchEdge<B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "remove-switch-edge";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let reshape = cx.mutate();
        let from = reshape
            .function()
            .basic_blocks()
            .find(|bb| bb.name().as_deref() == Some(self.from_name))
            .expect("`from` block is present");
        reshape.edit_switch(from.id())?.remove_successor(self.to)?;
        Ok(reshape.done())
    }
}

/// A `ReshapeCfg` pass that calls
/// `edit_switch(from.id())?.redirect_successor(old_to, new_to, ..)`, retargeting
/// the `from_name` block's case edge from `old_to` to `new_to` and seeding
/// `new_to`'s leading phis with the stashed `phi_values`.
struct RedirectSwitchEdge<B: ModuleBrand> {
    from_name: &'static str,
    old_to: BlockId<Dyn, B>,
    new_to: BlockId<Dyn, B>,
    phi_values: Vec<ValueId<B>>,
}

impl<B: ModuleBrand> FunctionPass<B> for RedirectSwitchEdge<B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "redirect-switch-edge";

    fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport>
    where
        'ctx: 'm,
        Self: 'ctx,
    {
        let reshape = cx.mutate();
        let from = reshape
            .function()
            .basic_blocks()
            .find(|bb| bb.name().as_deref() == Some(self.from_name))
            .expect("`from` block is present");
        reshape.edit_switch(from.id())?.redirect_successor(
            self.old_to,
            self.new_to,
            &self.phi_values,
        )?;
        Ok(reshape.done())
    }
}

/// The function id plus the `dflt` / `other` / `merge` `Dyn` block ids
/// returned by [`build_switch_merge`].
type SwitchMergeBuild<B> = IrResult<(
    crate::FunctionId<Dyn, B>,
    BlockId<Dyn, B>,
    BlockId<Dyn, B>,
    BlockId<Dyn, B>,
)>;

/// Build a `switch`-fed merge with a two-incoming phi, returning the function
/// and the `dflt` / `other` / `merge` `Dyn` labels.
///
/// ```text
/// entry(a): %e = add %a, 7
///           switch i32 %a, label %dflt [ i32 0, label %merge ; i32 1, label %other ]
/// dflt:     %d = add %a, 9 ; br %merge
/// other:    ret 0
/// merge:    %p = phi i32 [ %e, %entry ], [ %d, %dflt ] ; ret %p
/// ```
///
/// `merge`'s predecessors are `{entry (case 0), dflt (br)}`; removing the
/// `entry → merge` edge must drop the phi's `entry` incoming to stay coherent.
/// `dflt` is the switch default and ends in a plain `br` (a non-switch `from`);
/// `other` is the case-1 target — both feed the edge-op guard negatives below.
fn build_switch_merge<'ctx, B: crate::ModuleBrand + 'ctx>(
    m: &'ctx Module<B, crate::Unverified>,
) -> SwitchMergeBuild<B> {
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(m, "entry");
    let dflt = m.view(f).append_basic_block(m, "dflt");
    let other = m.view(f).append_basic_block(m, "other");
    let merge = m.view(f).append_basic_block(m, "merge");

    let entry_lbl = entry.id();
    let dflt_lbl = dflt.id();
    let other_lbl = other.id();
    let merge_lbl = merge.id();

    // entry: %e = add %a, 7 ; switch %a, default %dflt [ 0 -> merge, 1 -> other ]
    let b = IrBuilder::new_for::<Dyn>(m).position_at_end(entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let e = b.int_add(a, 7_i32, "e")?;
    let (_sealed, sw) = b.switch_dyn(a, dflt_lbl, "")?;
    sw.add_case(i32_ty.const_int(0_u32), merge_lbl)?
        .add_case(i32_ty.const_int(1_u32), other_lbl)?
        .finish();

    // dflt: %d = add %a, 9 ; br merge
    let b = IrBuilder::new_for::<Dyn>(m).position_at_end(dflt);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let d = b.int_add(a, 9_i32, "d")?;
    b.br(merge_lbl)?;

    // other: ret 0
    let b = IrBuilder::new_for::<Dyn>(m).position_at_end(other);
    b.ret(i32_ty.const_int(0_u32))?;

    // merge: %p = phi i32 [ %e, entry ], [ %d, dflt ] ; ret %p
    let b = IrBuilder::new_for::<Dyn>(m).position_at_end(merge);
    let p = b
        .view(b.int_phi::<i32, _>("p")?)
        .add_incoming(e, entry_lbl)?
        .add_incoming(d, dflt_lbl)?;
    b.ret(p.as_int_value())?;

    Ok((f, dflt_lbl, other_lbl, merge_lbl))
}

/// `remove_successor` drops the `entry → merge` switch case AND mechanically
/// removes `merge`'s phi incoming that named `entry`; the output re-verifies.
/// Without the phi maintenance, `merge`'s phi would still name `entry` (no
/// longer a predecessor) and `verify()` would fail with `PhiPredecessorMismatch`.
#[test]
fn remove_edge_drops_successor_phi_incoming() -> Result<(), IrError> {
    let m = crate::module_new!("remove-edge")?;
    let (f, _dflt_dyn, _other_dyn, merge_dyn) = build_switch_merge(&m)?;

    let verified = m.verify()?;
    let mut analyses = Analyses::new();
    let pass = RemoveSwitchEdge {
        from_name: "entry",
        to: merge_dyn,
    };
    let out = run_function_pass(pass, verified, f, &mut analyses)?;

    let reverified = out
        .verify()
        .expect("remove_successor output must re-verify");
    let printed = format!("{reverified}");
    assert!(
        printed.contains("[ %d, %dflt ]"),
        "merge's phi must keep the dflt incoming, got:\n{printed}"
    );
    assert!(
        !printed.contains(", %entry ]"),
        "merge's phi must have dropped the entry incoming, got:\n{printed}"
    );
    Ok(())
}

/// `edit_switch` rejects a `from` whose terminator is not a `switch` (here the
/// `dflt` block, which ends in a plain `br %merge`): the narrower only admits
/// `switch` terminators, so the non-`switch` edge op cannot even be spelled —
/// the narrow itself errs.
#[test]
fn edit_switch_rejects_non_switch_from() -> Result<(), IrError> {
    let m = crate::module_new!("remove-edge-non-switch")?;
    let (f, _dflt_dyn, _other_dyn, merge_dyn) = build_switch_merge(&m)?;

    let verified = m.verify()?;
    let mut analyses = Analyses::new();
    // `dflt` ends in `br %merge`, not a switch — the guard trips before the
    // successor check, so `merge` as `to` is fine.
    let pass = RemoveSwitchEdge {
        from_name: "dflt",
        to: merge_dyn,
    };
    let err = run_function_pass(pass, verified, f, &mut analyses)
        .err()
        .expect("edit_switch must reject a non-switch `from`");
    assert!(
        matches!(err, IrError::InvalidOperation { .. }),
        "expected InvalidOperation for a non-switch `from`, got: {err:?}"
    );
    Ok(())
}

/// `remove_successor` rejects dropping a `switch`'s default edge — a `switch`
/// must keep a default, so the `entry → dflt` default edge cannot be collapsed.
#[test]
fn remove_edge_rejects_default_edge() -> Result<(), IrError> {
    let m = crate::module_new!("remove-edge-default")?;
    let (f, dflt_dyn, _other_dyn, _merge_dyn) = build_switch_merge(&m)?;

    let verified = m.verify()?;
    let mut analyses = Analyses::new();
    // `dflt` is `entry`'s switch default; removing that edge is rejected.
    let pass = RemoveSwitchEdge {
        from_name: "entry",
        to: dflt_dyn,
    };
    let err = run_function_pass(pass, verified, f, &mut analyses)
        .err()
        .expect("remove_successor must reject dropping the switch default edge");
    assert!(
        matches!(err, IrError::InvalidOperation { .. }),
        "expected InvalidOperation for the switch default edge, got: {err:?}"
    );
    Ok(())
}

/// `redirect_successor` rejects a redirect whose `new_to` is already a successor
/// of `from`: a redirect mints a fresh edge, and if `from` already reached
/// `new_to` the new-target phi would gain an ambiguous second incoming for the
/// same predecessor.
#[test]
fn redirect_edge_rejects_already_reaches_new() -> Result<(), IrError> {
    let m = crate::module_new!("redirect-edge-already-reaches")?;
    let (f, _dflt_dyn, other_dyn, merge_dyn) = build_switch_merge(&m)?;

    let verified = m.verify()?;
    let mut analyses = Analyses::new();
    // `entry` already reaches `merge` (case 0); redirecting the `entry →
    // other` case-1 edge onto `merge` would double the edge. The guard
    // trips before any `phi_values` validation, so an empty slice is fine.
    let pass = RedirectSwitchEdge {
        from_name: "entry",
        old_to: other_dyn,
        new_to: merge_dyn,
        phi_values: vec![],
    };
    let err = run_function_pass(pass, verified, f, &mut analyses)
        .err()
        .expect("redirect_successor must reject a `new_to` already reached by `from`");
    assert!(
        matches!(err, IrError::InvalidOperation { .. }),
        "expected InvalidOperation when `from` already reaches `new_to`, got: {err:?}"
    );
    Ok(())
}

/// The function id plus the `shared` / `new` `Dyn` block ids returned by
/// [`build_switch_default_parallel`].
type SwitchDefaultParallelBuild<B> =
    IrResult<(crate::FunctionId<Dyn, B>, BlockId<Dyn, B>, BlockId<Dyn, B>)>;

/// Build a `switch` whose DEFAULT and case-0 both target `shared`, so `entry`
/// reaches `shared` through two parallel edges. A third predecessor `mid` gives
/// `shared` three predecessors total.
///
/// ```text
/// entry(a): %e = add %a, 7
///           switch i32 %a, label %shared [ i32 0, label %shared ; i32 1, label %mid ]
/// mid:      %mv = add %a, 3 ; br %shared
/// shared:   %p = phi i32 [ %e, %entry ], [ %e, %entry ], [ %mv, %mid ] ; ret %p
/// new:      ret 1   (phi-less redirect target)
/// ```
///
/// Redirecting the case-0 edge (`redirect_successor(shared, new)`) leaves the
/// DEFAULT still targeting `shared`, so `entry` survives as a predecessor of
/// `shared` through the default — `shared`'s phi must keep one `entry` incoming.
fn build_switch_default_parallel<'ctx, B: crate::ModuleBrand + 'ctx>(
    m: &'ctx Module<B, crate::Unverified>,
) -> SwitchDefaultParallelBuild<B> {
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(m, "entry");
    let mid = m.view(f).append_basic_block(m, "mid");
    let shared = m.view(f).append_basic_block(m, "shared");
    let new = m.view(f).append_basic_block(m, "new");

    let entry_lbl = entry.id();
    let mid_lbl = mid.id();
    let shared_lbl = shared.id();
    let new_lbl = new.id();

    // entry: %e = add %a, 7 ; switch %a, default %shared [ 0 -> shared, 1 -> mid ]
    let b = IrBuilder::new_for::<Dyn>(m).position_at_end(entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let e = b.int_add(a, 7_i32, "e")?;
    let (_sealed, sw) = b.switch_dyn(a, shared_lbl, "")?;
    sw.add_case(i32_ty.const_int(0_u32), shared_lbl)?
        .add_case(i32_ty.const_int(1_u32), mid_lbl)?
        .finish();

    // mid: %mv = add %a, 3 ; br shared
    let b = IrBuilder::new_for::<Dyn>(m).position_at_end(mid);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let mv = b.int_add(a, 3_i32, "mv")?;
    b.br(shared_lbl)?;

    // shared: %p = phi i32 [ %e, entry ] (default), [ %e, entry ] (case 0), [ %mv, mid ] ; ret %p
    let b = IrBuilder::new_for::<Dyn>(m).position_at_end(shared);
    let p = b
        .view(b.int_phi::<i32, _>("p")?)
        .add_incoming(e, entry_lbl)?
        .add_incoming(e, entry_lbl)?
        .add_incoming(mv, mid_lbl)?;
    b.ret(p.as_int_value())?;

    // new: ret 1  (no phi — a redirect onto it seeds an empty phi_values)
    let b = IrBuilder::new_for::<Dyn>(m).position_at_end(new);
    b.ret(i32_ty.const_int(1_u32))?;

    Ok((f, shared_lbl, new_lbl))
}

/// SURVIVING-PARALLEL-EDGE (switch redirect): redirecting the case-0 edge of a
/// `switch` whose DEFAULT also targets `shared` retargets only the case, leaving
/// the default at `shared`. `entry` STILL reaches `shared` through the default,
/// so `shared`'s phi must KEEP one `entry` incoming.
///
/// Before the fix, `redirect_slot` dropped every `entry` incoming from `shared`,
/// leaving `[ %mv, %mid ]` (1 entry) against 2 predecessors — `verify()` fails
/// with `PhiPredecessorMismatch`. (`remove_successor` cannot exercise this: it
/// rejects `old_to == default`, so a removed case can never parallel a surviving
/// default.)
#[test]
fn redirect_successor_keeps_surviving_default_parallel_phi_incoming() -> Result<(), IrError> {
    let m = crate::module_new!("switch-default-parallel")?;
    let (f, shared_dyn, new_dyn) = build_switch_default_parallel(&m)?;
    let verified = m.verify()?;
    let mut analyses = Analyses::new();
    let pass = RedirectSwitchEdge {
        from_name: "entry",
        old_to: shared_dyn,
        new_to: new_dyn,
        phi_values: vec![],
    };
    let out = run_function_pass(pass, verified, f, &mut analyses)?;

    let reverified = out.verify().expect(
        "redirect must re-verify: shared keeps one entry incoming for the surviving default",
    );
    let printed = format!("{reverified}");
    // shared retains one `entry` incoming (surviving default) plus its `mid`
    // incoming: two entries for two predecessors.
    assert!(
        printed.contains("[ %e, %entry ]"),
        "shared's phi must keep one entry incoming for the surviving default, got:\n{printed}"
    );
    assert!(
        printed.contains("[ %mv, %mid ]"),
        "shared's phi must keep its mid incoming, got:\n{printed}"
    );
    Ok(())
}
