//! Block-argument authoring surface for phis: `append_block_with_params`
//! creates a block whose parameters are operandless head-phis (the
//! Swift-SIL / MLIR block-argument shape). Incomings are supplied later by
//! branching to the block with the block-argument branch builders, so this
//! test only proves the block + head-phi(s) + returned param `Value`s exist
//! and print.

use llvmkit_ir::{IRBuilder, IntPredicate, IntValue, IrError, Linkage, Module};

/// A block appended with one `i32` parameter carries a single head-phi of
/// that type at its head, and the returned params vector surfaces that phi
/// as a `Value` of the right type.
#[test]
fn append_block_with_params_creates_head_phi() -> Result<(), IrError> {
    Module::with_new("block_args", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;

        // No positioning required: the block is created against `f`, not the
        // builder's cursor.
        let b = IRBuilder::new_for::<i32>(&m);
        let (hdr, params) = b.append_block_with_params(f, &[i32_ty.as_type()], "hdr")?;

        // (a) params vector: one entry, typed i32, backed by the head-phi.
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].ty(), i32_ty.as_type());

        // The returned block handle is the freshly-appended `hdr`.
        assert_eq!(hdr.label().as_value().name().as_deref(), Some("hdr"));

        // (b) the new block prints with a `phi i32` head-phi. `hdr` is the
        // only block carrying a phi, so its presence in the module text
        // proves the head-phi was materialised at the block's head.
        let text = format!("{m}");
        assert!(
            text.contains("phi i32"),
            "expected a `phi i32` head-phi in the printed module, got:\n{text}"
        );
        Ok(())
    })
}

/// Multiple params: `params[i]` is the i-th param type, and the head-phis
/// print in that same order — the ordering contract the block-argument branch
/// builders rely on to line up carried values with target params.
#[test]
fn append_block_with_params_preserves_param_order() -> Result<(), IrError> {
    Module::with_new("block_args_order", |m| {
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;

        let b = IRBuilder::new_for::<i32>(&m);
        let (_hdr, params) =
            b.append_block_with_params(f, &[i32_ty.as_type(), i64_ty.as_type()], "hdr")?;

        // params vector mirrors the requested type order.
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].ty(), i32_ty.as_type());
        assert_eq!(params[1].ty(), i64_ty.as_type());

        // The two head-phis print in the same order (i32 before i64).
        let text = format!("{m}");
        let i32_pos = text.find("phi i32").expect("i32 head-phi printed");
        let i64_pos = text.find("phi i64").expect("i64 head-phi printed");
        assert!(
            i32_pos < i64_pos,
            "head-phis must print in param order (i32 before i64), got:\n{text}"
        );
        Ok(())
    })
}

/// A `br` that carries a block argument seeds the target's leading head-phi
/// with a `(value, predecessor)` incoming: branching to `hdr(%x)` records
/// `[ %x, %entry ]` on `hdr`'s param-phi. The edge and the value move
/// together, so the completed phi verifies and prints the incoming pair.
#[test]
fn block_args_br_round_trips_and_verifies() -> Result<(), IrError> {
    Module::with_new("block_args_br", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");

        // hdr(%p: i32): a block with one i32 parameter (a head-phi).
        let bwp = IRBuilder::new_for::<i32>(&m);
        let (hdr, params) = bwp.append_block_with_params(f, &[i32_ty.as_type()], "hdr")?;
        let hdr_label = hdr.label();

        // entry: %x = add i32 %a, 1 ; br %hdr(%x)
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let x = b.build_int_add(a, 1_i32, "x")?;
        b.build_br_with_args(hdr_label, &[x.as_value()])?;

        // hdr: ret %p (the head-phi param carrying the branch argument).
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(hdr);
        let p: IntValue<i32> = params[0].try_into()?;
        b.build_ret(p)?;

        // The head-phi records `[ %x, %entry ]`. The param-phi is unnamed
        // (Task 1's `append_block_with_params` does not name individual
        // params), so assert on the load-bearing incoming pair rather than a
        // `%p =` label.
        let text = format!("{m}");
        assert!(
            text.contains("phi i32 [ %x, %entry ]"),
            "expected `phi i32 [ %x, %entry ]`, got:\n{text}"
        );

        // The completed phi verifies: its predecessor set {entry} matches the
        // CFG predecessors of hdr.
        m.verify()?;
        Ok(())
    })
}

/// A diamond whose two predecessors each carry their own value into a shared
/// 1-parameter merge block: `then` branches `merge(%vt)` and `else` branches
/// `merge(%ve)`, so the merge head-phi collects both incomings and verifies.
#[test]
fn block_args_cond_br_diamond_verifies() -> Result<(), IrError> {
    Module::with_new("block_args_diamond", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let then_bb = f.append_basic_block(&m, "then");
        let else_bb = f.append_basic_block(&m, "else");

        // merge(%p: i32): one i32 parameter reached from both arms.
        let bwp = IRBuilder::new_for::<i32>(&m);
        let (merge, params) = bwp.append_block_with_params(f, &[i32_ty.as_type()], "merge")?;
        let merge_label = merge.label();

        // entry: br (%a == 0) ? then : else
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let cond = b.build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, a, 0_i32, "c")?;
        b.build_cond_br(cond, &then_bb, &else_bb)?;

        // then: %vt = add %a, 10 ; br merge(%vt)
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(then_bb);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let vt = b.build_int_add(a, 10_i32, "vt")?;
        b.build_br_with_args(merge_label, &[vt.as_value()])?;

        // else: %ve = add %a, 20 ; br merge(%ve)
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(else_bb);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let ve = b.build_int_add(a, 20_i32, "ve")?;
        b.build_br_with_args(merge_label, &[ve.as_value()])?;

        // merge: ret %p
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(merge);
        let p: IntValue<i32> = params[0].try_into()?;
        b.build_ret(p)?;

        // Both incomings are present on the merge head-phi.
        let text = format!("{m}");
        assert!(
            text.contains("[ %vt, %then ]"),
            "expected `[ %vt, %then ]` incoming, got:\n{text}"
        );
        assert!(
            text.contains("[ %ve, %else ]"),
            "expected `[ %ve, %else ]` incoming, got:\n{text}"
        );

        m.verify()?;
        Ok(())
    })
}

/// `build_cond_br_with_args` carries a distinct argument down each edge into
/// the matching successor's parameter: `entry` feeds `%x` to `then(%pt)` and
/// `%y` to `else(%pe)`. Both param-blocks have `entry` as their sole
/// predecessor, so the module verifies and both incomings print.
#[test]
fn block_args_cond_br_with_args_carries_both_edges() -> Result<(), IrError> {
    Module::with_new("block_args_condbr", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");

        // then(%pt: i32) and else(%pe: i32): each a 1-parameter block.
        let bwp = IRBuilder::new_for::<i32>(&m);
        let (then_bb, then_params) =
            bwp.append_block_with_params(f, &[i32_ty.as_type()], "then")?;
        let (else_bb, else_params) =
            bwp.append_block_with_params(f, &[i32_ty.as_type()], "else")?;
        let then_label = then_bb.label();
        let else_label = else_bb.label();

        // entry: %x = add %a, 1 ; %y = add %a, 2 ;
        //        br (%a == 0) ? then(%x) : else(%y)
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let a: IntValue<i32> = f.param(0)?.try_into()?;
        let x = b.build_int_add(a, 1_i32, "x")?;
        let y = b.build_int_add(a, 2_i32, "y")?;
        let cond = b.build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, a, 0_i32, "c")?;
        b.build_cond_br_with_args(
            cond,
            then_label,
            &[x.as_value()],
            else_label,
            &[y.as_value()],
        )?;

        // then: ret %pt
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(then_bb);
        let pt: IntValue<i32> = then_params[0].try_into()?;
        b.build_ret(pt)?;

        // else: ret %pe
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(else_bb);
        let pe: IntValue<i32> = else_params[0].try_into()?;
        b.build_ret(pe)?;

        let text = format!("{m}");
        assert!(
            text.contains("[ %x, %entry ]"),
            "expected then-edge incoming `[ %x, %entry ]`, got:\n{text}"
        );
        assert!(
            text.contains("[ %y, %entry ]"),
            "expected else-edge incoming `[ %y, %entry ]`, got:\n{text}"
        );

        m.verify()?;
        Ok(())
    })
}

/// Arity guard: branching with fewer arguments than the target has parameters
/// fails at the branch builder (edge and values move together) rather than
/// deferring to a distant `verify()`.
#[test]
fn block_args_br_arity_mismatch_errors() -> Result<(), IrError> {
    Module::with_new("block_args_arity", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");

        // hdr has one param; branch with zero args → arity mismatch.
        let bwp = IRBuilder::new_for::<i32>(&m);
        let (hdr, _params) = bwp.append_block_with_params(f, &[i32_ty.as_type()], "hdr")?;
        let hdr_label = hdr.label();

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let res = b.build_br_with_args(hdr_label, &[]);
        assert!(
            matches!(
                res,
                Err(IrError::PhiArgArityMismatch {
                    expected: 1,
                    got: 0
                })
            ),
            "expected PhiArgArityMismatch {{ expected: 1, got: 0 }}, got: {res:?}"
        );
        Ok(())
    })
}

/// Type guard: an argument whose type differs from the target parameter is
/// rejected at the branch builder via the erased, type-checked incoming path.
#[test]
fn block_args_br_type_mismatch_errors() -> Result<(), IrError> {
    Module::with_new("block_args_type", |m| {
        let i32_ty = m.i32_type();
        let f64_ty = m.f64_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), f64_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");

        // hdr has one i32 param; branch carrying the f64 arg → type mismatch.
        let bwp = IRBuilder::new_for::<i32>(&m);
        let (hdr, _params) = bwp.append_block_with_params(f, &[i32_ty.as_type()], "hdr")?;
        let hdr_label = hdr.label();

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let arg_f64 = f.param(1)?; // f64 argument
        let res = b.build_br_with_args(hdr_label, &[arg_f64.as_value()]);
        assert!(
            matches!(res, Err(IrError::TypeMismatch { .. })),
            "expected TypeMismatch, got: {res:?}"
        );
        Ok(())
    })
}

/// `append_block_with_named_params` names each head-phi, so the printed IR
/// reads `%name = phi ...` instead of an anonymous slot — the capability that
/// lets block-argument authoring reproduce named-phi output byte-for-byte (the
/// hand-written factorial's `%acc`/`%i` loop-header phis).
#[test]
fn append_block_with_named_params_names_head_phis() -> Result<(), IrError> {
    Module::with_new("block_args_named", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");

        // hdr(%acc: i32, %i: i32): a two-parameter block whose head-phis are
        // named, unlike the anonymous `append_block_with_params`.
        let bwp = IRBuilder::new_for::<i32>(&m);
        let (hdr, params) = bwp.append_block_with_named_params(
            f,
            &[(i32_ty.as_type(), "acc"), (i32_ty.as_type(), "i")],
            "hdr",
        )?;
        let hdr_label = hdr.label();

        // entry: br hdr(1, 2) — seed both named head-phis.
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        b.build_br_with_args(
            hdr_label,
            &[
                i32_ty.const_int(1_i32).as_value(),
                i32_ty.const_int(2_i32).as_value(),
            ],
        )?;

        // hdr: ret %acc.
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(hdr);
        let acc: IntValue<i32> = params[0].try_into()?;
        b.build_ret(acc)?;

        let text = format!("{m}");
        assert!(
            text.contains("%acc = phi i32 [ 1, %entry ]"),
            "expected named `%acc` head-phi, got:\n{text}"
        );
        assert!(
            text.contains("%i = phi i32 [ 2, %entry ]"),
            "expected named `%i` head-phi, got:\n{text}"
        );
        m.verify()?;
        Ok(())
    })
}
