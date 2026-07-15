//! Regression coverage for the zero-incoming-phi round-trip hole.
//!
//! When [`FnReshape::remove_edge`](crate::pass_context) /
//! [`redirect_edge`](crate::pass_context) drops the *only* predecessor of a
//! block, that block's leading phis are left with zero incomings. A phi with no
//! incomings has no legal textual form (`%p = phi i32` with no `[ … ]` pairs),
//! which LLVM's own LL parser rejects, so the module no longer round-trips. The
//! edge ops now mirror LLVM `BasicBlock::removePredecessor`: an emptied phi is
//! RAUW'd with poison of its own type and erased.
//!
//! This case builds a `cond_br` whose then-arm target has `entry` as its ONLY
//! predecessor, so removing that edge empties the target's head phi — which
//! block arguments cannot express with a raw single-incoming phi, hence the
//! in-crate home.

use crate::{
    Analyses, BasicBlockLabel, Dyn, FnCx, FnReport, FunctionPass, IRBuilder, IntValue, IrError,
    IrResult, Linkage, Module, ModuleBrand, ReshapeCfg, run_function_pass,
};

/// A `ReshapeCfg` pass that removes the `from_name → to` edge. The `to` label is
/// stashed at build time (arena ids are stable across `verify()`).
struct RemoveEdge<'ctx, B: ModuleBrand + 'ctx> {
    from_name: &'static str,
    to: BasicBlockLabel<'ctx, Dyn, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for RemoveEdge<'ctx, B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "remove-edge-empty-phi";

    fn run(&mut self, cx: FnCx<'_, '_, 'ctx, B, ReshapeCfg, ()>) -> IrResult<FnReport> {
        let reshape = cx.mutate();
        let from = reshape
            .function()
            .basic_blocks()
            .find(|bb| bb.name().as_deref() == Some(self.from_name))
            .expect("`from` block is present");
        reshape.remove_edge(&from, &self.to)?;
        Ok(reshape.done())
    }
}

/// Build a `cond_br`-fed block whose ONLY predecessor is `entry`, so removing
/// the `entry → to` edge empties `to`'s single-incoming head phi. Returns the
/// function and `to`'s `Dyn` label.
///
/// ```text
/// entry(a): %x = add %a, 7
///           %c = icmp slt %a, 5
///           cond_br %c, to, other
/// to:       %p = phi i32 [ %x, entry ]
///           %u = add %p, 1 ; ret %u
/// other:    ret 0
/// ```
fn build_single_pred_phi<'ctx>(
    m: &Module<'ctx, crate::Brand<'ctx>, crate::Unverified>,
) -> IrResult<(crate::FunctionValue<'ctx, i32>, BasicBlockLabel<'ctx, Dyn>)> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block(m, "entry");
    let to = f.append_basic_block(m, "to");
    let other = f.append_basic_block(m, "other");

    let entry_lbl = entry.label();
    let to_lbl = to.label();
    let other_lbl = other.label();
    let to_dyn: BasicBlockLabel<Dyn> = to_lbl.as_value().try_into()?;

    // entry: %x = add %a, 7 ; %c = icmp slt %a, 5 ; cond_br %c, to, other
    let b = IRBuilder::new_for::<i32>(m).position_at_end(entry);
    let a: IntValue<i32> = f.param(0)?.try_into()?;
    let x = b.build_int_add(a, 7_i32, "x")?;
    let c = b.build_icmp_slt(a, 5_i32, "c")?;
    b.build_cond_br(c, to_lbl, other_lbl)?;

    // to: %p = phi i32 [ %x, entry ] ; %u = add %p, 1 ; ret %u
    let b = IRBuilder::new_for::<i32>(m).position_at_end(to);
    let p = b.build_int_phi::<i32, _>("p")?.add_incoming(x, entry_lbl)?;
    let u = b.build_int_add(p.as_int_value(), 1_i32, "u")?;
    b.build_ret(u)?;

    // other: ret 0
    let b = IRBuilder::new_for::<i32>(m).position_at_end(other);
    b.build_ret(i32_ty.const_int(0_u32))?;

    Ok((f, to_dyn))
}

/// Removing `entry → to` — `entry` being `to`'s only predecessor — empties
/// `to`'s head phi. The op must erase that phi (LLVM `removePredecessor`
/// parity), RAUW'ing its sole user onto poison, so the output re-verifies AND
/// round-trips (no bracket-less `phi i32` is printed).
///
/// Without the fix the phi survives with zero incomings: `verify()` still
/// accepts it (0 == 0) but the printed IR carries a `phi` LLVM's parser rejects,
/// so the `!contains("phi")` assertion below fails.
#[test]
fn remove_edge_emptying_phi_erases_it_with_poison() -> Result<(), IrError> {
    Module::with_new("remove-edge-empty-phi", |m| {
        let (f, to_dyn) = build_single_pred_phi(&m)?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let pass = RemoveEdge {
            from_name: "entry",
            to: to_dyn,
        };
        let out = run_function_pass(pass, verified, f, &mut analyses)?;

        let reverified = out
            .verify()
            .expect("remove_edge output must re-verify after emptying a phi");
        let printed = format!("{reverified}");
        // The emptied phi is erased entirely — never left as a bracket-less
        // `phi i32`, the shape LLVM's LL parser rejects. (Match the instruction
        // form, not the bare word: the module name also contains "phi".)
        assert!(
            !printed.contains("= phi"),
            "the emptied phi must have been erased, got:\n{printed}"
        );
        // Its sole user (`%u = add %p, 1`) was RAUW'd onto poison of the phi's
        // own type before the phi was detached.
        assert!(
            printed.contains("add i32 poison"),
            "the phi's user must now reference poison, got:\n{printed}"
        );
        Ok(())
    })
}
