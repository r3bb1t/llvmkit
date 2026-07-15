//! Raw-phi edge-op coverage relocated from `tests/analysis_preservation.rs`.
//!
//! These typed switch edge-edit cases (`edit_switch(..).remove_successor` /
//! `redirect_successor`) build a `switch`-fed
//! merge whose phi collects an incoming from a `switch` case edge — which the
//! block-argument authoring surface (`append_block_with_params` +
//! `build_*_with_args`) physically cannot express — so they keep the raw
//! `build_int_phi`/`add_incoming` API and run from inside the crate. They are
//! kept verbatim from their integration-test origin (only the `llvmkit_ir::`
//! paths are rewritten to `crate::`).

use crate::{
    Analyses, BasicBlockLabel, Dyn, FnCx, FnReport, FunctionPass, IRBuilder, IntValue, IrError,
    IrResult, Linkage, Module, ModuleBrand, ReshapeCfg, Value, run_function_pass,
};

/// A `ReshapeCfg` pass that calls `edit_switch(&from)?.remove_successor(&to)` on
/// the block named `from_name`, dropping its edge to `to`. The `to` label is
/// stashed at build time (arena ids are stable across `verify()`), mirroring how
/// `InsertMergePhi` stashes its incomings.
struct RemoveSwitchEdge<'ctx, B: ModuleBrand + 'ctx> {
    from_name: &'static str,
    to: BasicBlockLabel<'ctx, Dyn, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for RemoveSwitchEdge<'ctx, B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "remove-switch-edge";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let from = reshape
            .function()
            .basic_blocks()
            .find(|bb| bb.name().as_deref() == Some(self.from_name))
            .expect("`from` block is present");
        reshape.edit_switch(&from)?.remove_successor(&self.to)?;
        Ok(reshape.done())
    }
}

/// A `ReshapeCfg` pass that calls
/// `edit_switch(&from)?.redirect_successor(&old_to, &new_to, ..)`, retargeting
/// the `from_name` block's case edge from `old_to` to `new_to` and seeding
/// `new_to`'s leading phis with the stashed `phi_values`.
struct RedirectSwitchEdge<'ctx, B: ModuleBrand + 'ctx> {
    from_name: &'static str,
    old_to: BasicBlockLabel<'ctx, Dyn, B>,
    new_to: BasicBlockLabel<'ctx, Dyn, B>,
    phi_values: Vec<Value<'ctx, B>>,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for RedirectSwitchEdge<'ctx, B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "redirect-switch-edge";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let from = reshape
            .function()
            .basic_blocks()
            .find(|bb| bb.name().as_deref() == Some(self.from_name))
            .expect("`from` block is present");
        reshape.edit_switch(&from)?.redirect_successor(
            &self.old_to,
            &self.new_to,
            &self.phi_values,
        )?;
        Ok(reshape.done())
    }
}

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
#[allow(clippy::type_complexity)]
fn build_switch_merge<'ctx>(
    m: &Module<'ctx, crate::Brand<'ctx>, crate::Unverified>,
) -> IrResult<(
    crate::FunctionValue<'ctx, i32>,
    BasicBlockLabel<'ctx, Dyn>,
    BasicBlockLabel<'ctx, Dyn>,
    BasicBlockLabel<'ctx, Dyn>,
)> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block(m, "entry");
    let dflt = f.append_basic_block(m, "dflt");
    let other = f.append_basic_block(m, "other");
    let merge = f.append_basic_block(m, "merge");

    let entry_lbl = entry.label();
    let dflt_lbl = dflt.label();
    let other_lbl = other.label();
    let merge_lbl = merge.label();
    let merge_dyn: BasicBlockLabel<Dyn> = merge_lbl.as_value().try_into()?;
    let dflt_dyn: BasicBlockLabel<Dyn> = dflt_lbl.as_value().try_into()?;
    let other_dyn: BasicBlockLabel<Dyn> = other_lbl.as_value().try_into()?;

    // entry: %e = add %a, 7 ; switch %a, default %dflt [ 0 -> merge, 1 -> other ]
    let b = IRBuilder::new_for::<i32>(m).position_at_end(entry);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let e = b.build_int_add(a, 7_i32, "e")?;
    let (_sealed, sw) = b.build_switch(a, dflt_lbl, "")?;
    sw.add_case(i32_ty.const_int(0_u32), merge_lbl)?
        .add_case(i32_ty.const_int(1_u32), other_lbl)?
        .finish();

    // dflt: %d = add %a, 9 ; br merge
    let b = IRBuilder::new_for::<i32>(m).position_at_end(dflt);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let d = b.build_int_add(a, 9_i32, "d")?;
    b.build_br(merge_lbl)?;

    // other: ret 0
    let b = IRBuilder::new_for::<i32>(m).position_at_end(other);
    b.build_ret(i32_ty.const_int(0_u32))?;

    // merge: %p = phi i32 [ %e, entry ], [ %d, dflt ] ; ret %p
    let b = IRBuilder::new_for::<i32>(m).position_at_end(merge);
    let p = b
        .build_int_phi::<i32, _>("p")?
        .add_incoming(e, entry_lbl)?
        .add_incoming(d, dflt_lbl)?;
    b.build_ret(p.as_int_value())?;

    Ok((f, dflt_dyn, other_dyn, merge_dyn))
}

/// `remove_successor` drops the `entry → merge` switch case AND mechanically
/// removes `merge`'s phi incoming that named `entry`; the output re-verifies.
/// Without the phi maintenance, `merge`'s phi would still name `entry` (no
/// longer a predecessor) and `verify()` would fail with `PhiPredecessorMismatch`.
#[test]
fn remove_edge_drops_successor_phi_incoming() -> Result<(), IrError> {
    Module::with_new("remove-edge", |m| {
        let (f, _dflt_dyn, _other_dyn, merge_dyn) = build_switch_merge(&m)?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let pass = RemoveSwitchEdge {
            from_name: "entry",
            to: merge_dyn,
        };
        let out = run_function_pass(pass, verified, f, &mut analyses)?;

        let reverified = out.verify().expect("remove_edge output must re-verify");
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
    })
}

/// `edit_switch` rejects a `from` whose terminator is not a `switch` (here the
/// `dflt` block, which ends in a plain `br %merge`): the narrower only admits
/// `switch` terminators, so the non-`switch` edge op cannot even be spelled —
/// the narrow itself errs.
#[test]
fn edit_switch_rejects_non_switch_from() -> Result<(), IrError> {
    Module::with_new("remove-edge-non-switch", |m| {
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
    })
}

/// `remove_successor` rejects dropping a `switch`'s default edge — a `switch`
/// must keep a default, so the `entry → dflt` default edge cannot be collapsed.
#[test]
fn remove_edge_rejects_default_edge() -> Result<(), IrError> {
    Module::with_new("remove-edge-default", |m| {
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
    })
}

/// `redirect_successor` rejects a redirect whose `new_to` is already a successor
/// of `from`: a redirect mints a fresh edge, and if `from` already reached
/// `new_to` the new-target phi would gain an ambiguous second incoming for the
/// same predecessor.
#[test]
fn redirect_edge_rejects_already_reaches_new() -> Result<(), IrError> {
    Module::with_new("redirect-edge-already-reaches", |m| {
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
    })
}
