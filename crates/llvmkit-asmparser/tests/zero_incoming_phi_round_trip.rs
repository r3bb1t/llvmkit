//! Round-trip regression for the zero-incoming-phi hole in
//! `FnReshape::remove_edge`.
//!
//! Removing a block's only incoming edge empties its head phi. Before the fix,
//! the op left that phi in place with zero incomings; it prints as
//! `%p = phi i32` with no `[ … ]` pairs, a form LLVM's own LL parser rejects —
//! so the module no longer round-trips. The fix mirrors LLVM
//! `BasicBlock::removePredecessor`: the emptied phi is replaced with poison (of
//! its own type) and erased, so the printed IR round-trips.
//!
//! The IR is built through the public block-argument surface (a block parameter
//! *is* a head phi) and the reshape pass machinery — no crate-internal phi
//! builder — then reparsed with this crate's parser.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::{
    Analyses, BasicBlockLabel, Dyn, FnCx, FnReport, FunctionPass, IRBuilder, IntValue, IrError,
    IrResult, Linkage, Module, ModuleBrand, ReshapeCfg, run_function_pass,
};

/// A `ReshapeCfg` pass that removes the `from_name → to` edge.
struct RemoveEdge<'ctx, B: ModuleBrand + 'ctx> {
    from_name: &'static str,
    to: BasicBlockLabel<'ctx, Dyn, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> FunctionPass<'ctx, B> for RemoveEdge<'ctx, B> {
    type Access = ReshapeCfg;
    type Requires = ();
    const NAME: &'static str = "remove-edge-empty-phi-rt";

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

/// Build IR whose `to` block has `entry` as its ONLY predecessor, remove the
/// `entry → to` edge (emptying `to`'s block-parameter phi), and return the
/// printed output.
///
/// ```text
/// entry(a): %c = icmp eq %a, 0
///           br %c ? to(%a) : other()
/// to(%p):   ret %p            ; preds { entry }  -- the sole edge
/// other:    ret 0
/// ```
fn build_and_empty_phi() -> IrResult<String> {
    Module::with_new("empty-phi-build", |m| -> IrResult<String> {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let other = f.append_basic_block(&m, "other");
        // to(%p: i32): reached ONLY from entry.
        let (to_bb, to_params) =
            IRBuilder::new_for::<i32>(&m).append_block_with_params(f, &[i32_ty.as_type()], "to")?;
        let to_lbl = to_bb.label();
        let other_lbl = other.label();
        let to_dyn: BasicBlockLabel<Dyn> = to_lbl.as_value().try_into()?;

        // entry: %c = icmp eq %a, 0 ; br %c ? to(%a) : other()
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let c = b.build_icmp_eq(a, 0_i32, "c")?;
        b.build_cond_br_with_args(c, to_lbl, &[a.as_value()], other_lbl, &[])?;

        // to: ret %p
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(to_bb);
        let p: IntValue<i32> = to_params[0].try_into()?;
        b.build_ret(p)?;

        // other: ret 0
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(other);
        b.build_ret(i32_ty.const_int(0_u32))?;

        let verified = m.verify()?;
        let mut analyses = Analyses::new();
        let pass = RemoveEdge {
            from_name: "entry",
            to: to_dyn,
        };
        let out = run_function_pass(pass, verified, f, &mut analyses)?;
        let reverified = out.verify().expect("remove_edge output must re-verify");
        Ok(format!("{reverified}"))
    })
}

/// The printed output of an edge removal that empties a phi must reparse. This
/// is the exact property the bug broke: a bracket-less `%p = phi i32` has no
/// legal LLVM textual form. (This crate's parser is intentionally lenient about
/// zero-input phis — see `phi_real_incomings::zero_input_phi_still_parses` — so
/// the reparse alone would accept the buggy output too; the precise guard is
/// that the emptied phi is gone entirely, which the `= phi` assertion enforces.)
#[test]
fn emptied_phi_output_round_trips() -> Result<(), IrError> {
    let printed = build_and_empty_phi()?;

    // The emptied phi was erased (RAUW'd onto poison), not left bracket-less.
    assert!(
        !printed.contains("= phi"),
        "the emptied phi must be erased, not left bracket-less, got:\n{printed}"
    );
    assert!(
        printed.contains("ret i32 poison"),
        "the phi's user must have been RAUW'd onto poison, got:\n{printed}"
    );

    // The printed IR reparses and re-verifies through this crate's parser.
    Module::with_new("empty-phi-reparse", |m2| {
        Parser::new(printed.as_bytes(), &m2)
            .expect("lexer primes")
            .parse_module()
            .expect("emptied-phi output must reparse");
        let reparsed = m2.verify().expect("reparsed IR must verify");
        let reprinted = format!("{reparsed}");
        assert!(
            !reprinted.contains("= phi"),
            "reparsed IR must still be phi-free, got:\n{reprinted}"
        );
    });
    Ok(())
}
