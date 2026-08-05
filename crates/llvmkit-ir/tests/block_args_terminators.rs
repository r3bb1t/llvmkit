//! Every terminator that can reach a **parameterised** block either carries
//! that block's arguments or does not build.
//!
//! `tests/block_args.rs` covers the `br` / `cond_br` half of the
//! block-argument authoring surface. This file covers the two things that
//! surface was missing:
//!
//! 1. the **guard** — a *plain* `br` / `cond_br` / `switch` / `invoke` /
//!    `indirectbr` edge into a block created with block parameters is rejected
//!    at the builder (`IrError::PhiArgArityMismatch`), instead of silently
//!    seeding nothing and leaving an incomplete phi for `Module::verify()`;
//! 2. the **argument-carrying forms** for the two terminators that never had
//!    one — `switch_with_args` / `switch_dyn_with_args` and
//!    `invoke_with_args` / `invoke_dyn_with_args`.
//!
//! The guard keys on "was this block *created* with parameters", not on "does
//! it contain phis" — so the `.ll` parser, the auto-SSA builder, and pass-side
//! phi insertion, which all seed their phis through their own checked paths,
//! are untouched. `plain_branch_into_auto_ssa_phi_block_still_builds` pins
//! that distinction.

use llvmkit_ir::{
    Dyn, IntPredicate, IntValue, IrBuilder, IrError, Linkage, PointerValue, SsaBuilder, SsaState,
    module_new,
};

// --------------------------------------------------------------------------
// The guard: a plain edge into a parameterised block is rejected
// --------------------------------------------------------------------------

/// A plain `br` into a parameterised block fails **at the branch**, with
/// the same `PhiArgArityMismatch` a wrong argument count gets from
/// `br_with_args` — it used to build fine and leave the head-phi one
/// incoming short for a distant `Module::verify()` to report. Nothing is
/// emitted: the check runs before the terminator is appended.
#[test]
fn plain_br_into_param_block_errors() -> Result<(), IrError> {
    let m = module_new!("plain_br_guard")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");

    let bwp = IrBuilder::new_for::<Dyn>(&m);
    let (hdr, _params) = bwp.append_block_with_params(m.view(f), &[i32_ty.as_type()], "hdr")?;
    let hdr_label = hdr.id();

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let res = b.br(hdr_label);
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

    // Rejected before emission: no `br` reached the instruction list.
    let text = format!("{m}");
    assert!(
        !text.contains("br label %hdr"),
        "a rejected branch must leave no half-formed terminator, got:\n{text}"
    );
    Ok(())
}

/// Both `cond_br` arms are guarded, not just the first: a parameterised
/// then-target and a parameterised else-target are each rejected on their own.
#[test]
fn plain_cond_br_into_param_block_errors_on_either_arm() -> Result<(), IrError> {
    let m = module_new!("plain_cond_br_guard")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let then_entry = m.view(f).append_basic_block(&m, "then_entry");
    let else_entry = m.view(f).append_basic_block(&m, "else_entry");
    let plain = m.view(f).append_basic_block(&m, "plain");
    let plain_label = plain.id();

    let bwp = IrBuilder::new_for::<Dyn>(&m);
    let (param_bb, _params) =
        bwp.append_block_with_params(m.view(f), &[i32_ty.as_type()], "par")?;
    let param_label = param_bb.id();

    // then-arm parameterised.
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(then_entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let c = b.int_cmp::<i32, _, _, _>(IntPredicate::Eq, a, 0_i32, "c")?;
    let res = b.cond_br(c, param_label, plain_label);
    assert!(
        matches!(res, Err(IrError::PhiArgArityMismatch { expected: 1, .. })),
        "then-arm into a param block must be rejected, got: {res:?}"
    );

    // else-arm parameterised.
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(else_entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let c = b.int_cmp::<i32, _, _, _>(IntPredicate::Eq, a, 0_i32, "c2")?;
    let res = b.cond_br(c, plain_label, param_label);
    assert!(
        matches!(res, Err(IrError::PhiArgArityMismatch { expected: 1, .. })),
        "else-arm into a param block must be rejected, got: {res:?}"
    );
    Ok(())
}

/// A `switch` reaches a parameterised block two ways — through its default
/// edge and through a case edge — and both are guarded: `switch` rejects
/// the default, `SwitchInst::add_case` rejects the case.
#[test]
fn plain_switch_into_param_block_errors_on_default_and_case() -> Result<(), IrError> {
    let m = module_new!("plain_switch_guard")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let default_entry = m.view(f).append_basic_block(&m, "default_entry");
    let case_entry = m.view(f).append_basic_block(&m, "case_entry");
    let plain = m.view(f).append_basic_block(&m, "plain");
    let plain_label = plain.id();

    let bwp = IrBuilder::new_for::<Dyn>(&m);
    let (param_bb, _params) =
        bwp.append_block_with_params(m.view(f), &[i32_ty.as_type()], "par")?;
    let param_label = param_bb.id();

    // Default edge.
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(default_entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let res = b.switch::<i32, _, _, _>(a, param_label, "");
    assert!(
        matches!(
            res.map(|(_, _)| ()),
            Err(IrError::PhiArgArityMismatch {
                expected: 1,
                got: 0
            })
        ),
        "a switch default into a param block must be rejected"
    );

    // Case edge.
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(case_entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let (_sealed, open) = b.switch::<i32, _, _, _>(a, plain_label, "")?;
    let res = open.add_case(0_i32, param_label);
    assert!(
        matches!(
            res.map(|_| ()),
            Err(IrError::PhiArgArityMismatch {
                expected: 1,
                got: 0
            })
        ),
        "a switch case into a param block must be rejected"
    );
    Ok(())
}

/// Both `invoke` edges are mandatory and neither carries arguments in the
/// plain form, so a parameterised normal *or* unwind destination is rejected.
#[test]
fn plain_invoke_into_param_block_errors_on_either_edge() -> Result<(), IrError> {
    let m = module_new!("plain_invoke_guard")?;
    let void_ty = m.void_type();
    let i32_ty = m.i32_type();
    let callee_ty = m.fn_type(
        void_ty.as_type(),
        Vec::<llvmkit_ir::Type<'_, _>>::new(),
        false,
    );
    let callee = m.add_function_dyn("callee", callee_ty, Linkage::External)?;
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let normal_entry = m.view(f).append_basic_block(&m, "normal_entry");
    let unwind_entry = m.view(f).append_basic_block(&m, "unwind_entry");
    let plain = m.view(f).append_basic_block(&m, "plain");
    let plain_label = plain.id();

    let bwp = IrBuilder::new_for::<Dyn>(&m);
    let (param_bb, _params) =
        bwp.append_block_with_params(m.view(f), &[i32_ty.as_type()], "par")?;
    let param_label = param_bb.id();

    // Parameterised normal destination.
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(normal_entry);
    let res = b.invoke_dyn::<Dyn, _, llvmkit_ir::Value<'_, _>, _, _, _>(
        m.view(callee),
        Vec::new(),
        param_label,
        plain_label,
        "",
    );
    assert!(
        matches!(
            res.map(|(_, _)| ()),
            Err(IrError::PhiArgArityMismatch {
                expected: 1,
                got: 0
            })
        ),
        "an invoke normal edge into a param block must be rejected"
    );

    // Parameterised unwind destination.
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(unwind_entry);
    let res = b.invoke_dyn::<Dyn, _, llvmkit_ir::Value<'_, _>, _, _, _>(
        m.view(callee),
        Vec::new(),
        plain_label,
        param_label,
        "",
    );
    assert!(
        matches!(
            res.map(|(_, _)| ()),
            Err(IrError::PhiArgArityMismatch {
                expected: 1,
                got: 0
            })
        ),
        "an invoke unwind edge into a param block must be rejected"
    );
    Ok(())
}

/// `indirectbr` has no argument-carrying form — the address picks the
/// destination at run time, so there is nothing to hang a per-edge argument
/// list on. A parameterised destination is therefore rejected outright, the
/// restriction the block-argument design called for.
#[test]
fn indirectbr_destination_into_param_block_errors() -> Result<(), IrError> {
    let m = module_new!("indirectbr_guard")?;
    let void_ty = m.void_type();
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(void_ty.as_type(), [ptr_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");

    let bwp = IrBuilder::new_for::<Dyn>(&m);
    let (param_bb, _params) =
        bwp.append_block_with_params(m.view(f), &[i32_ty.as_type()], "par")?;
    let param_label = param_bb.id();

    let addr: PointerValue<'_, _> = m.view(f).param(0)?.try_into()?;
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let (_sealed, ibr) = b.indirectbr(addr, "")?;
    let res = ibr.add_destination(param_label);
    assert!(
        matches!(
            res.map(|_| ()),
            Err(IrError::PhiArgArityMismatch {
                expected: 1,
                got: 0
            })
        ),
        "an indirectbr destination that is a param block must be rejected"
    );
    Ok(())
}

/// `callbr` is in the same position as `indirectbr`: its indirect edges are
/// taken by inline assembly at run time, so it has no argument-carrying form
/// and a parameterised destination — default or indirect — is rejected.
#[test]
fn plain_callbr_into_param_block_errors() -> Result<(), IrError> {
    let m = module_new!("callbr_guard")?;
    let void_ty = m.void_type();
    let i32_ty = m.i32_type();
    let callee_ty = m.fn_type(
        void_ty.as_type(),
        Vec::<llvmkit_ir::Type<'_, _>>::new(),
        false,
    );
    let callee = m.add_function_dyn("callee", callee_ty, Linkage::External)?;
    let fn_ty = m.fn_type(
        void_ty.as_type(),
        Vec::<llvmkit_ir::Type<'_, _>>::new(),
        false,
    );
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let default_entry = m.view(f).append_basic_block(&m, "default_entry");
    let indirect_entry = m.view(f).append_basic_block(&m, "indirect_entry");
    let plain = m.view(f).append_basic_block(&m, "plain");
    let plain_label = plain.id();

    let bwp = IrBuilder::new_for::<Dyn>(&m);
    let (param_bb, _params) =
        bwp.append_block_with_params(m.view(f), &[i32_ty.as_type()], "par")?;
    let param_label = param_bb.id();

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(default_entry);
    let res = b.callbr::<Dyn, _, llvmkit_ir::Value<'_, _>, _, _, _, _, _>(
        m.view(callee),
        Vec::new(),
        param_label,
        [plain_label],
        "",
    );
    assert!(
        matches!(
            res.map(|(_, _)| ()),
            Err(IrError::PhiArgArityMismatch {
                expected: 1,
                got: 0
            })
        ),
        "a callbr default into a param block must be rejected"
    );

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(indirect_entry);
    let res = b.callbr::<Dyn, _, llvmkit_ir::Value<'_, _>, _, _, _, _, _>(
        m.view(callee),
        Vec::new(),
        plain_label,
        [param_label],
        "",
    );
    assert!(
        matches!(
            res.map(|(_, _)| ()),
            Err(IrError::PhiArgArityMismatch {
                expected: 1,
                got: 0
            })
        ),
        "a callbr indirect destination that is a param block must be rejected"
    );
    Ok(())
}

/// The guard keys on **block parameters**, not on "the block has phis": a
/// block whose phis were created by the auto-SSA engine is still a legal plain
/// `br` / `cond_br` target, because those phis are completed at `seal_block`
/// rather than by branch arguments. Same for the `.ll` parser's back-edges
/// (covered by the parser round-trip corpus) and pass-inserted phis.
#[test]
fn plain_branch_into_auto_ssa_phi_block_still_builds() -> Result<(), IrError> {
    let m = module_new!("auto_ssa_not_parameterised")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let mut state = SsaState::for_function(&m, m.view(f))?;
    let mut b = SsaBuilder::for_function(&m, m.view(f), &mut state)?;
    let entry = b.create_block("entry");
    let loop_bb = b.create_block("loop");
    let exit = b.create_block("exit");
    let counter = b.declare_int_var::<i32, _>("counter");

    b.switch_to_block(entry)?;
    let n: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    b.def_int_var(counter, n)?;
    b.br(loop_bb)?;

    b.switch_to_block(loop_bb)?;
    // `loop` is unsealed, so this read mints an OPERANDLESS phi at its head —
    // the exact shape the naive "does the target start with a phi?" guard
    // would have rejected the back-edge below for.
    let current = b.use_int_var(counter)?;
    let next = b.ins()?.int_sub(current, 1_i32, "next")?;
    let done = b
        .ins()?
        .int_cmp::<i32, _, _, _>(IntPredicate::Eq, next, 0_i32, "done")?;
    b.def_int_var(counter, next)?;
    b.cond_br(done, exit, loop_bb)?;
    b.seal_block(loop_bb)?;

    b.switch_to_block(exit)?;
    b.seal_block(exit)?;
    let read = b.use_int_var(counter)?;
    b.ret(read)?;
    b.finish()?;

    let text = format!("{m}");
    assert!(
        text.contains("phi i32 ["),
        "the auto-SSA loop header must carry a completed phi, got:\n{text}"
    );
    m.verify_borrowed()?;
    Ok(())
}

// --------------------------------------------------------------------------
// `switch_with_args`
// --------------------------------------------------------------------------

/// A SIL-style loop authored entirely through `switch` block arguments: the
/// header's two parameters are seeded from `entry` by `br_with_args` and
/// from the latch by the switch's **default** edge, while the switch's one
/// **case** edge carries the accumulator out to a parameterised `exit`. Every
/// phi is complete by construction and the module verifies clean.
#[test]
fn switch_with_args_authors_a_sil_style_loop() -> Result<(), IrError> {
    let m = module_new!("switch_block_args_loop")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");

    let bwp = IrBuilder::new_for::<Dyn>(&m);
    let (loop_bb, loop_params) = bwp.append_block_with_named_params(
        m.view(f),
        &[(i32_ty.as_type(), "i"), (i32_ty.as_type(), "acc")],
        "loop",
    )?;
    let (exit_bb, exit_params) =
        bwp.append_block_with_named_params(m.view(f), &[(i32_ty.as_type(), "r")], "exit")?;
    let loop_label = loop_bb.id();
    let exit_label = exit_bb.id();

    // entry: br loop(0, 1)
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    b.br_with_args(
        loop_label,
        &[
            i32_ty.const_int(0_i32).into_erased(),
            i32_ty.const_int(1_i32).into_erased(),
        ],
    )?;

    // loop(%i, %acc):
    //   %next_i = add %i, 1 ; %next_acc = mul %acc, 2
    //   switch %i, default loop(%next_i, %next_acc) [ 10 -> exit(%acc) ]
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(loop_bb);
    let i: IntValue<'_, i32, _> = loop_params[0].try_into()?;
    let acc: IntValue<'_, i32, _> = loop_params[1].try_into()?;
    let next_i = b.int_add(i, 1_i32, "next_i")?;
    let next_acc = b.int_mul(acc, 2_i32, "next_acc")?;
    b.switch_with_args(
        i,
        (
            loop_label,
            &[m.view(next_i).into_erased(), m.view(next_acc).into_erased()][..],
        ),
        [(10_i32, exit_label, &[loop_params[1]][..])],
        "",
    )?;

    // exit(%r): ret %r
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(exit_bb);
    let r: IntValue<'_, i32, _> = exit_params[0].try_into()?;
    b.ret(r)?;

    let text = format!("{m}");
    assert!(
        text.contains("%i = phi i32 [ 0, %entry ], [ %next_i, %loop ]"),
        "the loop counter phi must collect the entry and default-edge incomings, got:\n{text}"
    );
    assert!(
        text.contains("%acc = phi i32 [ 1, %entry ], [ %next_acc, %loop ]"),
        "the accumulator phi must collect both incomings, got:\n{text}"
    );
    assert!(
        text.contains("%r = phi i32 [ %acc, %loop ]"),
        "the case edge must seed exit's parameter, got:\n{text}"
    );
    m.verify()?;
    Ok(())
}

/// Arity is checked per edge, on the default and on each case alike, before
/// any incoming is recorded and before the switch is emitted.
#[test]
fn switch_with_args_arity_mismatch_errors() -> Result<(), IrError> {
    let m = module_new!("switch_block_args_arity")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let default_entry = m.view(f).append_basic_block(&m, "default_entry");
    let case_entry = m.view(f).append_basic_block(&m, "case_entry");
    let plain = m.view(f).append_basic_block(&m, "plain");
    let plain_label = plain.id();

    let bwp = IrBuilder::new_for::<Dyn>(&m);
    let (param_bb, _params) =
        bwp.append_block_with_params(m.view(f), &[i32_ty.as_type()], "par")?;
    let param_label = param_bb.id();

    // Default edge: one parameter, zero arguments.
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(default_entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let res = b.switch_with_args(a, (param_label, &[]), [(0_i32, plain_label, &[][..])], "");
    assert!(
        matches!(
            res.map(|(_, _)| ()),
            Err(IrError::PhiArgArityMismatch {
                expected: 1,
                got: 0
            })
        ),
        "a default edge missing its argument must be rejected"
    );

    // Case edge: one parameter, two arguments.
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(case_entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let two = [a.into_erased(), a.into_erased()];
    let res = b.switch_with_args(a, (plain_label, &[]), [(0_i32, param_label, &two[..])], "");
    assert!(
        matches!(
            res.map(|(_, _)| ()),
            Err(IrError::PhiArgArityMismatch {
                expected: 1,
                got: 2
            })
        ),
        "a case edge carrying too many arguments must be rejected"
    );
    Ok(())
}

/// A block argument whose type differs from its target parameter is rejected
/// at the call site, the same way `br_with_args` rejects it.
#[test]
fn switch_with_args_type_mismatch_errors() -> Result<(), IrError> {
    let m = module_new!("switch_block_args_type")?;
    let i32_ty = m.i32_type();
    let f64_ty = m.f64_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), f64_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let plain = m.view(f).append_basic_block(&m, "plain");
    let plain_label = plain.id();

    let bwp = IrBuilder::new_for::<Dyn>(&m);
    let (param_bb, _params) =
        bwp.append_block_with_params(m.view(f), &[i32_ty.as_type()], "par")?;
    let param_label = param_bb.id();

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let wrong = [m.view(f).param(1)?.into_erased()]; // f64 into an i32 parameter
    let res = b.switch_with_args(
        a,
        (plain_label, &[]),
        [(0_i32, param_label, &wrong[..])],
        "",
    );
    assert!(
        matches!(res.map(|(_, _)| ()), Err(IrError::TypeMismatch { .. })),
        "a mistyped case argument must be rejected"
    );
    Ok(())
}

/// The width-erased twin seeds the same edges; its case values are checked
/// against the condition's width at run time rather than at compile time.
#[test]
fn switch_dyn_with_args_seeds_default_and_case() -> Result<(), IrError> {
    let m = module_new!("switch_dyn_block_args")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");

    let bwp = IrBuilder::new_for::<Dyn>(&m);
    let (dflt, dflt_params) =
        bwp.append_block_with_named_params(m.view(f), &[(i32_ty.as_type(), "dp")], "dflt")?;
    let (case_bb, case_params) =
        bwp.append_block_with_named_params(m.view(f), &[(i32_ty.as_type(), "cp")], "case")?;
    let dflt_label = dflt.id();
    let case_label = case_bb.id();

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let dflt_arg = [a.into_erased()];
    let case_arg = [i32_ty.const_int(7_i32).into_erased()];
    b.switch_dyn_with_args(
        a,
        (dflt_label, &dflt_arg),
        [(i32_ty.const_int(0_i32), case_label, &case_arg[..])],
        "",
    )?;

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(dflt);
    let dp: IntValue<'_, i32, _> = dflt_params[0].try_into()?;
    b.ret(dp)?;
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(case_bb);
    let cp: IntValue<'_, i32, _> = case_params[0].try_into()?;
    b.ret(cp)?;

    let text = format!("{m}");
    assert!(
        text.contains("%dp = phi i32 [ %0, %entry ]"),
        "the default edge must seed its target's parameter, got:\n{text}"
    );
    assert!(
        text.contains("%cp = phi i32 [ 7, %entry ]"),
        "the case edge must seed its target's parameter, got:\n{text}"
    );
    m.verify()?;
    Ok(())
}

// --------------------------------------------------------------------------
// `invoke_with_args`
// --------------------------------------------------------------------------

/// Both `invoke` edges are mandatory, so both carry an argument list: the
/// normal and unwind destinations are each parameterised here and each is
/// seeded from the invoking block. The module verifies clean.
#[test]
fn invoke_with_args_seeds_both_edges() -> Result<(), IrError> {
    let m = module_new!("invoke_block_args")?;
    let i32_ty = m.i32_type();
    let callee = m.add_typed_function::<(), (), _>("callee", Linkage::External)?;
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");

    let bwp = IrBuilder::new_for::<Dyn>(&m);
    let (normal, normal_params) =
        bwp.append_block_with_named_params(m.view(f), &[(i32_ty.as_type(), "np")], "normal")?;
    let (unwind, unwind_params) =
        bwp.append_block_with_named_params(m.view(f), &[(i32_ty.as_type(), "up")], "unwind")?;
    let normal_label = normal.id();
    let unwind_label = unwind.id();

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let x = b.int_add(a, 1_i32, "x")?;
    let carried = [m.view(x).into_erased()];
    b.invoke_with_args(
        m.view(callee),
        (),
        (normal_label, &carried),
        (unwind_label, &carried),
        "",
    )?;

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(normal);
    let np: IntValue<'_, i32, _> = normal_params[0].try_into()?;
    b.ret(np)?;
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(unwind);
    let up: IntValue<'_, i32, _> = unwind_params[0].try_into()?;
    b.ret(up)?;

    let text = format!("{m}");
    assert!(
        text.contains("%np = phi i32 [ %x, %entry ]"),
        "the normal edge must seed its destination's parameter, got:\n{text}"
    );
    assert!(
        text.contains("%up = phi i32 [ %x, %entry ]"),
        "the unwind edge must seed its destination's parameter, got:\n{text}"
    );
    m.verify()?;
    Ok(())
}

/// The erased-callee twin seeds the same two edges, and a wrong argument count
/// on either is rejected at the call site before the `invoke` is emitted.
#[test]
fn invoke_dyn_with_args_seeds_edges_and_checks_arity() -> Result<(), IrError> {
    let m = module_new!("invoke_dyn_block_args")?;
    let void_ty = m.void_type();
    let i32_ty = m.i32_type();
    let callee_ty = m.fn_type(
        void_ty.as_type(),
        Vec::<llvmkit_ir::Type<'_, _>>::new(),
        false,
    );
    let callee = m.add_function_dyn("callee", callee_ty, Linkage::External)?;
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let bad_entry = m.view(f).append_basic_block(&m, "bad_entry");
    let plain = m.view(f).append_basic_block(&m, "plain");
    let plain_label = plain.id();

    let bwp = IrBuilder::new_for::<Dyn>(&m);
    let (normal, normal_params) =
        bwp.append_block_with_named_params(m.view(f), &[(i32_ty.as_type(), "np")], "normal")?;
    let normal_label = normal.id();

    // Happy path: the normal edge carries the parameter, the unwind edge is an
    // ordinary block and carries nothing.
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let carried = [a.into_erased()];
    b.invoke_dyn_with_args::<Dyn, _, llvmkit_ir::Value<'_, _>, _, _, _>(
        m.view(callee),
        Vec::new(),
        (normal_label, &carried),
        (plain_label, &[]),
        "",
    )?;

    // Wrong arity on the normal edge.
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(bad_entry);
    let res = b.invoke_dyn_with_args::<Dyn, _, llvmkit_ir::Value<'_, _>, _, _, _>(
        m.view(callee),
        Vec::new(),
        (normal_label, &[]),
        (plain_label, &[]),
        "",
    );
    assert!(
        matches!(
            res.map(|(_, _)| ()),
            Err(IrError::PhiArgArityMismatch {
                expected: 1,
                got: 0
            })
        ),
        "an invoke edge missing its block argument must be rejected"
    );

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(normal);
    let np: IntValue<'_, i32, _> = normal_params[0].try_into()?;
    b.ret(np)?;
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(plain);
    b.ret(i32_ty.const_int(0_i32))?;

    let text = format!("{m}");
    assert!(
        text.contains("%np = phi i32 [ %0, %entry ]"),
        "the normal edge must seed its destination's parameter, got:\n{text}"
    );
    Ok(())
}
