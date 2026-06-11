//! Inline assembly as a `call` callee. Exercises the `InlineAsm` value
//! (mirrors LLVM's `InlineAsm`) and the `asm "...", "..."` printer path in
//! the asm-writer's `CallInst` arm.

use llvmkit_ir::{AsmDialect, IRBuilder, IrError, Linkage, Module, Type};

/// `%r = call i64 asm sideeffect "add $1, $0", "=r,r,r"(i64 %a, i64 %b)`.
/// Mirrors `AsmWriter.cpp::writeAsOperandInternal(Value*)` inline-asm arm.
/// Asserts the `asm` keyword, the `sideeffect` flag, the constraint
/// string, the call result type (`i64`, not the callee's `ptr` type), and
/// the argument operands all print.
#[test]
fn inline_asm_call_with_side_effects() -> Result<(), IrError> {
    let m = Module::new("inline_asm");
    let i64_ty = m.i64_type();

    // The function the host body lives in: i64 @add_via_asm(i64 %a, i64 %b).
    let host_ty = m.fn_type(i64_ty, [i64_ty.as_type(), i64_ty.as_type()], false);
    let host = m.add_function::<i64>("add_via_asm", host_ty, Linkage::External)?;
    let entry = host.append_basic_block("entry");
    let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);

    let a = host.param(0).expect("param 0");
    let bb = host.param(1).expect("param 1");

    // i64 (i64, i64) inline asm with sideeffect.
    let asm_fn_ty = m.fn_type(i64_ty, [i64_ty.as_type(), i64_ty.as_type()], false);
    let asm = m.inline_asm(
        asm_fn_ty,
        "add $1, $0",
        "=r,r,r",
        /* has_side_effects */ true,
        /* is_align_stack */ false,
        AsmDialect::ATT,
    );

    let r = b.build_inline_asm_call::<i64, _, _>(asm, [a, bb], "r")?;
    b.build_ret(r.return_int_value())?;

    let text = format!("{m}");

    // The callee prints as the `asm` form, with the `sideeffect` keyword.
    assert!(
        text.contains(r#"call i64 asm sideeffect "add"#),
        "expected `call i64 asm sideeffect \"add`:\n{text}"
    );
    // The constraint string, immediately followed by the `(i64 ...)` args.
    assert!(
        text.contains(r#", "=r,r,r"(i64 "#),
        "expected constraint + arg list:\n{text}"
    );
    // The asm value's pointer type must NOT leak into the printed call.
    assert!(
        !text.contains("call ptr asm"),
        "call result type must be the asm return type, not ptr:\n{text}"
    );
    // No `@name` callee operand for the asm form.
    assert!(
        !text.contains("call i64 @"),
        "inline-asm callee must not print as @name:\n{text}"
    );
    Ok(())
}

/// A no-`sideeffect` inline-asm call: the `sideeffect` keyword must be
/// absent, but the bare `asm` keyword is still present.
/// Mirrors `AsmWriter.cpp::writeAsOperandInternal(Value*)` inline-asm arm.
#[test]
fn inline_asm_call_without_side_effects() -> Result<(), IrError> {
    let m = Module::new("inline_asm_pure");
    let i32_ty = m.i32_type();

    let host_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let host = m.add_function::<i32>("neg_via_asm", host_ty, Linkage::External)?;
    let entry = host.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);

    let x = host.param(0).expect("param 0");

    let asm_fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let asm = m.inline_asm(
        asm_fn_ty,
        "neg $0",
        "=r,0",
        /* has_side_effects */ false,
        /* is_align_stack */ false,
        AsmDialect::ATT,
    );

    let r = b.build_inline_asm_call::<i32, _, _>(asm, [x], "r")?;
    b.build_ret(r.return_int_value())?;

    let text = format!("{m}");

    assert!(
        text.contains(r#"call i32 asm "neg"#),
        "expected `call i32 asm \"neg`:\n{text}"
    );
    assert!(
        !text.contains("sideeffect"),
        "no-sideeffect asm must not print the sideeffect keyword:\n{text}"
    );
    Ok(())
}

/// A multi-line AT&T asm template: the embedded newline must be escaped in
/// the printed string (mirrors `module asm` / string-constant escaping),
/// not emitted as a literal line break inside the quotes.
/// Mirrors `AsmWriter.cpp::writeAsOperandInternal(Value*)` string escaping for
/// inline-asm callee operands.
#[test]
fn inline_asm_multiline_escapes_newline() -> Result<(), IrError> {
    let m = Module::new("inline_asm_multiline");
    let void_ty = m.void_type();

    let host_ty = m.fn_type(void_ty.as_type(), Vec::<Type>::new(), false);
    let host = m.add_function::<()>("fence_via_asm", host_ty, Linkage::External)?;
    let entry = host.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);

    let asm_fn_ty = m.fn_type(void_ty.as_type(), Vec::<Type>::new(), false);
    // Two instructions separated by a newline in the template.
    let asm = m.inline_asm(
        asm_fn_ty,
        "nop\nnop",
        "",
        /* has_side_effects */ true,
        /* is_align_stack */ false,
        AsmDialect::ATT,
    );

    b.build_inline_asm_call::<(), _, _>(asm, Vec::<llvmkit_ir::Value>::new(), "")?;
    b.build_ret_void();

    let text = format!("{m}");

    // The newline is escaped as `\0a` (LLVM's printEscapedString hex
    // escape; case-insensitive check so the assertion is robust).
    let lower = text.to_ascii_lowercase();
    assert!(
        lower.contains(r#""nop\0anop""#),
        "embedded newline must be escaped as \\0A, not a literal break:\n{text}"
    );
    // And there must be no raw newline inside the quoted asm template.
    assert!(
        !text.contains("\"nop\nnop\""),
        "asm template must not contain a literal newline:\n{text}"
    );
    Ok(())
}

/// Indirect call return marker must match the explicit function type.
/// Mirrors `IRBuilder::CreateCall(FunctionType*, Value*, ...)`, where the
/// result type is determined by `FunctionType`.
#[test]
fn indirect_call_rejects_wrong_return_marker() -> Result<(), IrError> {
    let m = Module::new("indirect_marker");
    let void_ty = m.void_type();
    let ptr_ty = m.ptr_type(0);
    let host_ty = m.fn_type(void_ty.as_type(), [ptr_ty.as_type()], false);
    let host = m.add_function::<()>("host", host_ty, Linkage::External)?;
    let entry = host.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let callee_ptr = llvmkit_ir::PointerValue::try_from(host.param(0).expect("callee ptr"))?;
    let callee_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let err = b
        .build_indirect_call::<i64, _, _>(
            callee_ty,
            callee_ptr,
            Vec::<llvmkit_ir::Value>::new(),
            "bad",
        )
        .expect_err("void function type cannot produce i64 call marker");
    assert!(
        err.to_string().contains("return"),
        "unexpected error: {err}"
    );
    Ok(())
}

/// Label constraints are only legal with `callbr`, not ordinary `call`.
/// Mirrors `Verifier::verifyInlineAsmCall` label constraint check.
#[test]
fn inline_asm_call_rejects_label_constraint() -> Result<(), IrError> {
    let m = Module::new("asm_label_constraint");
    let void_ty = m.void_type();
    let host_ty = m.fn_type(void_ty.as_type(), Vec::<Type>::new(), false);
    let host = m.add_function::<()>("host", host_ty, Linkage::External)?;
    let entry = host.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let asm_ty = m.fn_type(void_ty.as_type(), Vec::<Type>::new(), false);
    let asm = m.inline_asm(asm_ty, "", "!i", false, false, AsmDialect::ATT);
    b.build_inline_asm_call::<(), _, _>(asm, Vec::<llvmkit_ir::Value>::new(), "")?;
    b.build_ret_void();
    let err = m
        .verify_borrowed()
        .expect_err("ordinary call with label constraint must fail");
    assert!(
        err.to_string()
            .contains("Label constraints can only be used with callbr"),
        "unexpected error: {err}"
    );
    Ok(())
}

/// Intel-dialect asm prints the `inteldialect` keyword after `asm`.
/// Mirrors `AsmWriter.cpp::writeAsOperandInternal(Value*)` inline-asm dialect
/// keyword printing.
#[test]
fn inline_asm_intel_dialect_keyword() -> Result<(), IrError> {
    let m = Module::new("inline_asm_intel");
    let i64_ty = m.i64_type();

    let host_ty = m.fn_type(i64_ty, [i64_ty.as_type()], false);
    let host = m.add_function::<i64>("id_via_asm", host_ty, Linkage::External)?;
    let entry = host.append_basic_block("entry");
    let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);
    let x = host.param(0).expect("param 0");

    let asm_fn_ty = m.fn_type(i64_ty, [i64_ty.as_type()], false);
    let asm = m.inline_asm(
        asm_fn_ty,
        "mov $0, $1",
        "=r,r",
        /* has_side_effects */ false,
        /* is_align_stack */ false,
        AsmDialect::Intel,
    );

    let r = b.build_inline_asm_call::<i64, _, _>(asm, [x], "r")?;
    b.build_ret(r.return_int_value())?;

    let text = format!("{m}");
    assert!(
        text.contains(r#"asm inteldialect "mov"#),
        "Intel dialect must print the inteldialect keyword:\n{text}"
    );
    Ok(())
}
