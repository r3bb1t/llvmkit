//! Forward-reference substrate: the un-uniqued placeholder constant and the
//! RAUW that retires it.
//!
//! **No upstream counterpart** (Doctrine D11). Upstream's forward-reference
//! sentinels are heap objects — `new Argument(Ty)` in
//! `LLParser::PerFunctionState::getVal`, `createGlobalFwdRef` for a
//! `@`-reference — whose distinctness is pointer identity and whose
//! retirement is `Value::replaceAllUsesWith` followed by `deleteValue`.
//! llvmkit has neither raw pointers nor deletion, so the same protocol is
//! rebuilt on an arena slot plus a linear handle. That representation choice
//! is what these tests pin. The *parser behaviour* the sentinels exist for —
//! which `.ll` texts are accepted, and with what diagnostics — is covered by
//! the ported fixtures in `llvmkit-asmparser`.

use llvmkit_ir::{BinaryOpcode, IntBinOpFlags, IrBuilder, IrError, Linkage, NoFolder, module_new};

/// A placeholder standing in an instruction operand is repointed at the real
/// definition when it arrives. This is the shape `%a = add i32 %b, 1` takes
/// when `%b` is defined further down the block: the instruction is built
/// eagerly against the sentinel, exactly as upstream builds it against the
/// `Argument` that `PerFunctionState::getVal` minted.
#[test]
fn rauw_repoints_instruction_operands() -> Result<(), IrError> {
    let m = module_new!("fwd-instruction")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);

    let placeholder = m.forward_ref_value_placeholder(i32_ty.as_type())?;
    let sum = b.int_binop_erased(
        BinaryOpcode::Add,
        placeholder.as_value(),
        i32_ty.const_int(1),
        IntBinOpFlags::new(),
        "sum",
    )?;
    assert!(
        format!("{m}").contains("%sum = add i32 <forward reference>, 1"),
        "the unresolved operand must be visible before RAUW:\n{m}"
    );

    let sum_value = b.view(sum);
    placeholder.replace_all_uses_with(m.view(f).param(0)?.as_erased())?;
    b.ret(sum_value)?;

    assert!(
        format!("{m}").contains("%sum = add i32 %0, 1"),
        "the operand must now name the argument:\n{m}"
    );
    Ok(())
}

/// A placeholder embedded in a *uniqued* constant cannot be rewritten in
/// place without breaking that constant's structural identity, so the
/// aggregate is re-interned and the aggregate's own users are repointed at
/// the new node. Mirrors the `ConstantExpr::getWithOperands` discipline
/// upstream applies when RAUW reaches a constant user.
#[test]
fn rauw_reinterns_constant_users() -> Result<(), IrError> {
    let m = module_new!("fwd-constant")?;
    let i32_ty = m.i32_type();
    let array_ty = m.array_type_n::<i32, 2>();
    let fn_ty = m.function_type(array_ty.as_type(), [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);

    let placeholder = m.forward_ref_value_placeholder(i32_ty.as_type())?;
    let aggregate =
        array_ty.const_array([placeholder.as_constant(), i32_ty.const_int(7).into()])?;
    b.ret(aggregate)?;
    assert!(
        format!("{m}").contains("ret [2 x i32] [i32 <forward reference>, i32 7]"),
        "the unresolved element must be visible before RAUW:\n{m}"
    );

    placeholder.replace_all_uses_with(i32_ty.const_int(5).as_erased())?;

    assert!(
        format!("{m}").contains("ret [2 x i32] [i32 5, i32 7]"),
        "the return operand must follow the re-interned aggregate:\n{m}"
    );
    assert_eq!(
        array_ty.const_array([i32_ty.const_int(5), i32_ty.const_int(7)])?,
        array_ty.const_array([i32_ty.const_int(5), i32_ty.const_int(7)])?,
        "re-interning must not have minted a second node for [5, 7]"
    );
    Ok(())
}

/// The replacement may itself be a user of the placeholder. Upstream reaches
/// this whenever a value forward-references itself —
/// `%x = phi i32 [ 0, %entry ], [ %x, %loop ]`, where `setInstName` RAUWs the
/// sentinel to the very instruction that consumed it. The walker must survive
/// rewriting a use-list entry into a self-reference rather than looping.
#[test]
fn rauw_survives_a_self_reference() -> Result<(), IrError> {
    let m = module_new!("fwd-self")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);

    let placeholder = m.forward_ref_value_placeholder(i32_ty.as_type())?;
    let sum = b.int_binop_erased(
        BinaryOpcode::Add,
        placeholder.as_value(),
        i32_ty.const_int(1),
        IntBinOpFlags::new(),
        "sum",
    )?;
    let sum_value = b.view(sum);
    placeholder.replace_all_uses_with(sum_value)?;
    b.ret(sum_value)?;

    assert!(
        format!("{m}").contains("%sum = add i32 %sum, 1"),
        "the instruction must now reference itself:\n{m}"
    );
    Ok(())
}

/// A sentinel carries the type its first use demanded, so resolving it to a
/// definition of a different type is a type error rather than a silent
/// retype. Upstream reports the same disagreement as
/// `instruction forward referenced with type '<T>'`
/// (`PerFunctionState::setInstName`); the parser renders that text on top of
/// this check.
#[test]
fn rauw_rejects_a_type_mismatch() -> Result<(), IrError> {
    let m = module_new!("fwd-mismatch")?;
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let placeholder = m.forward_ref_value_placeholder(i32_ty.as_type())?;
    assert!(
        placeholder
            .replace_all_uses_with(i64_ty.const_int(1).as_erased())
            .is_err(),
        "a differently typed definition must not silently retype the sentinel"
    );
    Ok(())
}

/// Constants may only be built from constants, so a placeholder that some
/// constant embeds cannot be resolved to an instruction result. Upstream
/// cannot reach this state at all — its function-local sentinel is an
/// `Argument`, which no constant can name — so llvmkit reports it rather
/// than interning a constant with a non-constant operand.
#[test]
fn rauw_inside_a_constant_rejects_a_non_constant_replacement() -> Result<(), IrError> {
    let m = module_new!("fwd-constant-mismatch")?;
    let i32_ty = m.i32_type();
    let array_ty = m.array_type_n::<i32, 2>();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    m.view(f).append_basic_block(&m, "entry");

    let placeholder = m.forward_ref_value_placeholder(i32_ty.as_type())?;
    let _aggregate =
        array_ty.const_array([placeholder.as_constant(), i32_ty.const_int(7).into()])?;
    assert!(
        placeholder
            .replace_all_uses_with(m.view(f).param(0)?.as_erased())
            .is_err(),
        "an argument cannot stand in a constant aggregate"
    );
    Ok(())
}
