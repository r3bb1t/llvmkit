//! `call` instructions with `metadata` arguments and post-hoc function
//! attributes.
//!
//! Exercises the `MetadataAsValue` bridge (mirrors LLVM's
//! `MetadataAsValue`) that lets a metadata node be passed as a `call`
//! argument — the shape the named-register intrinsics
//! `@llvm.read_register` / `@llvm.write_register` require — plus the
//! `FunctionValue::add_attribute` setter for forward-declared functions.

use llvmkit_ir::{
    AttrIndex, AttrKind, Attribute, Dyn, InstructionView, IrBuilder, IrError, Linkage,
    MetadataAttachmentKind, NoFolder, Ptr, VerifierRule, module_new,
};

fn assert_line(text: &str, expected: &str) {
    for line in text.lines() {
        if line == expected {
            return;
        }
    }
    panic!("missing line `{expected}` in:\n{text}");
}

fn assert_line_with_fragments(text: &str, fragments: &[&str]) {
    'lines: for line in text.lines() {
        for fragment in fragments {
            if line.find(fragment).is_none() {
                continue 'lines;
            }
        }
        return;
    }
    panic!("missing line with fragments {fragments:?} in:\n{text}");
}

fn assert_no_line_with_fragment(text: &str, fragment: &str) {
    for line in text.lines() {
        assert!(
            line.find(fragment).is_none(),
            "unexpected line containing `{fragment}` in:\n{text}"
        );
    }
}

/// Build the read/write named-register intrinsics, emit calls whose
/// argument is the same `metadata` node, and assert the printed node body is
/// exactly `!{!"rsp"}`. Mirrors `AsmWriter.cpp::writeAsOperandInternal(Value*)`
/// for `MetadataAsValue` call operands.
#[test]
fn call_with_metadata_argument() -> Result<(), IrError> {
    let m = module_new!("named_registers")?;
    let i64_ty = m.i64_type();

    // !N = !{!"rsp"}  — a tuple whose only operand is the register name.
    let s = m.metadata_string("rsp");
    let node = m.metadata_tuple([s])?;
    let md = m.metadata_as_value(node)?;

    // declare i64  @llvm.read_register.i64(metadata)
    let read = m.get_or_insert_intrinsic_declaration_by_name("llvm.read_register.i64")?;
    // declare void @llvm.write_register.i64(metadata, i64)
    let write = m.get_or_insert_intrinsic_declaration_by_name("llvm.write_register.i64")?;

    // define i64 @get_sp() { %rsp = call ...; call void ...; ret i64 %rsp }
    let host_ty = m.fn_type(i64_ty, Vec::<llvmkit_ir::Type<'_, _>>::new(), false);
    let host = m.add_function_dyn("get_sp", host_ty, Linkage::External)?;
    let entry = m.view(host).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);

    let rsp = b.call_dyn(read, [md], "rsp")?;
    let rsp_val: llvmkit_ir::IntValue<'_, i64, _> = b
        .view(rsp)
        .return_value()
        .expect("read_register returns value")
        .try_into()?;
    b.call_dyn(write, [md, rsp_val.as_erased()], "")?;
    b.ret(rsp_val)?;

    let text = format!("{m}");
    let mut read_line = None;
    let mut write_line = None;
    for line in text.lines() {
        if line
            .find("call i64 @llvm.read_register.i64(metadata ")
            .is_some()
        {
            read_line = Some(line);
        }
        if line
            .find("call void @llvm.write_register.i64(metadata ")
            .is_some()
        {
            write_line = Some(line);
        }
    }
    let read_line = read_line.unwrap_or_else(|| panic!("missing read-register call:\n{text}"));
    let write_line = write_line.unwrap_or_else(|| panic!("missing write-register call:\n{text}"));
    let read_md = read_line
        .split_once("@llvm.read_register.i64(metadata ")
        .and_then(|(_, tail)| tail.strip_suffix(')'))
        .unwrap_or_else(|| panic!("missing read metadata operand:\n{text}"));
    let write_md = write_line
        .split_once("@llvm.write_register.i64(metadata ")
        .and_then(|(_, tail)| tail.split_once(", i64 ").map(|(md, _)| md))
        .unwrap_or_else(|| panic!("missing write metadata operand:\n{text}"));
    assert_eq!(
        read_md, write_md,
        "calls must share one metadata node:\n{text}"
    );
    let expected_node = format!("{read_md} = !{{!\"rsp\"}}");
    assert_line(&text, &expected_node);
    Ok(())
}

/// A forward-declared function gains attributes after creation via
/// `FunctionValue::add_attribute` / `set_string_attribute`, and they
/// print on the definition. Mirrors `Function::addFnAttr` usage where a
/// declaration is created first and decorated as its body is emitted.
#[test]
fn post_construction_function_attributes() -> Result<(), IrError> {
    let m = module_new!("attrs")?;
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(
        void_ty.as_type(),
        Vec::<llvmkit_ir::Type<'_, _>>::new(),
        false,
    );

    // Forward declaration via `add_function_dyn` (no builder).
    let f = m.add_function_dyn("trampoline", fn_ty, Linkage::External)?;
    // Body is defined later; decorate the existing value.
    m.view(f).add_attribute(
        &m,
        AttrIndex::Function,
        Attribute::enum_attr(AttrKind::NoRedZone).expect("flag attribute"),
    );
    m.view(f).add_attribute(
        &m,
        AttrIndex::Function,
        Attribute::enum_attr(AttrKind::Naked).expect("flag attribute"),
    );
    m.view(f)
        .set_string_attribute(&m, AttrIndex::Function, "frame-pointer", "all");

    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    b.ret_void()?;

    let text = format!("{m}");
    assert_line_with_fragments(
        &text,
        &[
            "define void @trampoline()",
            "noredzone",
            "naked",
            r#""frame-pointer"="all""#,
        ],
    );
    Ok(())
}

/// `metadata_as_value` uniques by metadata node (mirrors LLVM's
/// `MetadataAsValue::get`): wrapping the same node twice yields the
/// identical `Value`, so value identity/equality stays meaningful and
/// the use-list is not fragmented across duplicates.
#[test]
fn metadata_as_value_is_uniqued() {
    let m = module_new!("u").expect("fresh module");
    let s = m.metadata_string("rsp");
    let node = m.metadata_tuple([s]).expect("native operand");
    let a = m.metadata_as_value(node).expect("native node");
    let b = m.metadata_as_value(node).expect("native node");
    assert_eq!(a, b, "same metadata node must yield the same Value");
}

/// A bare MDString used through `MetadataAsValue` prints inline as `!"rsp"`,
/// not as a numbered top-level `!N = !"rsp"` definition.
/// Mirrors `AsmWriter.cpp::writeAsOperandInternal(Metadata*)` MDString arm.
#[test]
fn metadata_string_as_value_prints_inline() -> Result<(), IrError> {
    let m = module_new!("md_string_value")?;
    let void_ty = m.void_type();
    let md_ty = m.metadata_type();
    let fn_ty = m.fn_type(void_ty.as_type(), [md_ty.as_type()], false);
    let g = m.add_function_dyn("g", fn_ty, Linkage::External)?;
    let host_ty = m.fn_type(
        void_ty.as_type(),
        Vec::<llvmkit_ir::Type<'_, _>>::new(),
        false,
    );
    let f = m.add_function_dyn("f", host_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let s = m.metadata_string("rsp");
    let md = m.metadata_as_value(s)?;
    b.call_dyn(g, [md], "")?;
    b.ret_void()?;

    let text = format!("{m}");
    assert_line(&text, r#"  call void @g(metadata !"rsp")"#);
    assert_no_line_with_fragment(&text, r#" = !"rsp""#);
    Ok(())
}

/// Named metadata operands are MDNode references; MDStrings inside those nodes
/// still print inline in the referenced tuple body.
/// Mirrors `AsmWriter.cpp::writeAllMDNodes` emitting only MDNodes while
/// `AsmWriter.cpp::writeAsOperandInternal(Metadata*)` prints MDStrings inline.
#[test]
fn string_referenced_by_named_metadata_is_not_dangling() {
    let m = module_new!("d").expect("fresh module");
    let s = m.metadata_string("x");
    let tuple = m.metadata_tuple([s]).expect("native operand");
    let idx = m.get_or_insert_named_metadata("my.named");
    m.named_metadata_add_operand(idx, tuple).unwrap();

    let text = format!("{m}");
    assert_line(&text, r#"!0 = !{!"x"}"#);
    assert_line(&text, "!my.named = !{!0}");
}

/// Mirrors `llvm/test/Analysis/ValueTracking/known-bits-from-range-md.ll`
/// and `llvm/test/Verifier/absolute_symbol.ll` typed integer metadata operands.
#[test]
fn metadata_constant_tuple_prints_typed_constants() {
    let m = module_new!("mdc").expect("fresh module");
    let i64_ty = m.i64_type();
    let one = m
        .metadata_constant(i64_ty.const_int(1_i64))
        .expect("native constant");
    let five = m
        .metadata_constant(i64_ty.const_int(5_i64))
        .expect("native constant");
    let tuple = m.metadata_tuple([one, five]).expect("native operands");
    let idx = m.get_or_insert_named_metadata("ranges");
    m.named_metadata_add_operand(idx, tuple).unwrap();

    let text = format!("{m}");
    assert_line(&text, "!0 = !{i64 1, i64 5}");
    assert_line(&text, "!ranges = !{!0}");
}

/// Mirrors `Verifier::visitRangeMetadata` accepting well-formed `!range`
/// metadata on loads.
#[test]
fn range_metadata_on_load_verifies_and_prints() -> Result<(), IrError> {
    let m = module_new!("range_ok")?;
    let i8_ty = m.i8_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(i8_ty, [ptr_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let p: llvmkit_ir::PointerValue<'_, _> = m.view(f).param(0)?.try_into()?;
    let ld = b.int_load::<i8, _, _>(p, "v")?;
    let lo = m.metadata_constant(i8_ty.const_int(0x10_u8))?;
    let hi = m.metadata_constant(i8_ty.const_int(0x20_u8))?;
    let range = m.metadata_tuple([lo, hi])?;
    let inst = InstructionView::try_from(b.view(ld).as_erased())?;
    inst.set_metadata(&m, MetadataAttachmentKind::Range, range)?;
    b.ret(ld)?;

    m.verify_borrowed()?;
    let text = format!("{m}");
    assert_line(&text, "  %v = load i8, ptr %0, align 1, !range !0");
    assert_line(&text, "!0 = !{i8 16, i8 32}");
    Ok(())
}

/// Mirrors `Verifier::verifyRangeLikeMetadata` rejecting an unfinished
/// range operand list.
#[test]
fn range_metadata_rejects_odd_operand_count() -> Result<(), IrError> {
    let m = module_new!("range_odd")?;
    let i8_ty = m.i8_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(i8_ty, [ptr_ty.as_type()], false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let p: llvmkit_ir::PointerValue<'_, _> = m.view(f).param(0)?.try_into()?;
    let ld = b.int_load::<i8, _, _>(p, "v")?;
    let lo = m.metadata_constant(i8_ty.const_int(0x10_u8))?;
    let range = m.metadata_tuple([lo])?;
    let inst = InstructionView::try_from(b.view(ld).as_erased())?;
    inst.set_metadata(&m, MetadataAttachmentKind::Range, range)?;
    b.ret(ld)?;

    let err = m
        .verify_borrowed()
        .expect_err("odd range metadata must fail");
    assert!(matches!(
        err,
        IrError::VerifierFailure {
            rule: VerifierRule::RangeMetadataMalformed,
            ..
        }
    ));
    Ok(())
}

/// Mirrors `llvm/test/Verifier/range-2.ll` allowing `!range` on call and
/// invoke return values.
#[test]
fn range_metadata_on_call_and_invoke_verifies() -> Result<(), IrError> {
    let m = module_new!("range_call_invoke_ok")?;
    let i8_ty = m.i8_type();
    let ptr_ty = m.ptr_type(0);
    let callee = m
        .add_typed_function::<i8, (Ptr,), _>("callee", Linkage::External)?
        .as_function();
    let lo = m.metadata_constant(i8_ty.const_int(0_i8))?;
    let hi = m.metadata_constant(i8_ty.const_int(1_i8))?;
    let range = m.metadata_tuple([lo, hi])?;

    let call_host_ty = m.fn_type(i8_ty, [ptr_ty.as_type()], false);
    let call_host = m.add_function_dyn("call_host", call_host_ty, Linkage::External)?;
    let call_entry = m.view(call_host).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(call_entry);
    let p: llvmkit_ir::PointerValue<'_, _> = m.view(call_host).param(0)?.try_into()?;
    let call = b.view(b.call_dyn(callee, [p.as_erased()], "v")?);
    call.as_view()
        .set_metadata(&m, MetadataAttachmentKind::Range, range)?;
    let ret = call.return_int_value();
    b.ret(ret)?;

    let invoke_host_ty = m.fn_type(i8_ty, [ptr_ty.as_type()], false);
    let invoke_host = m.add_function_dyn("invoke_host", invoke_host_ty, Linkage::External)?;
    let entry = m.view(invoke_host).append_basic_block(&m, "entry");
    let normal = m.view(invoke_host).append_basic_block(&m, "normal");
    let unwind = m.view(invoke_host).append_basic_block(&m, "unwind");
    let normal_label = normal.id();
    let unwind_label = unwind.id();
    let p: llvmkit_ir::PointerValue<'_, _> = m.view(invoke_host).param(0)?.try_into()?;
    let (_entry, invoke) = IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .invoke_dyn(
            m.view(callee),
            [p.as_erased()],
            normal_label,
            unwind_label,
            "v",
        )?;
    invoke
        .as_view()
        .set_metadata(&m, MetadataAttachmentKind::Range, range)?;
    let invoke_value: llvmkit_ir::IntValue<'_, i8, _> = invoke.to_erased().try_into()?;
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(normal)
        .ret(invoke_value)?;
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(unwind)
        .ret(i8_ty.const_zero())?;

    m.verify_borrowed()
}

/// Mirrors `Verifier::visitInstruction` rejecting `!range` on non
/// load/call/invoke instructions.
#[test]
fn range_metadata_rejects_non_load_call_invoke_user() -> Result<(), IrError> {
    let m = module_new!("range_bad_user")?;
    let i8_ty = m.i8_type();
    let fn_ty = m.fn_type(i8_ty, Vec::<llvmkit_ir::Type<'_, _>>::new(), false);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let add = b.int_add::<i8, _, _, _>(i8_ty.const_int(1_u8), i8_ty.const_int(2_u8), "sum")?;
    let lo = m.metadata_constant(i8_ty.const_int(0x10_u8))?;
    let hi = m.metadata_constant(i8_ty.const_int(0x20_u8))?;
    let range = m.metadata_tuple([lo, hi])?;
    let inst = InstructionView::try_from(b.view(add).as_erased())?;
    inst.set_metadata(&m, MetadataAttachmentKind::Range, range)?;
    b.ret(add)?;

    let err = m
        .verify_borrowed()
        .expect_err("range metadata on add must fail");
    assert!(matches!(
        err,
        IrError::VerifierFailure {
            rule: VerifierRule::RangeMetadataInvalidAttachment,
            ..
        }
    ));
    Ok(())
}

/// Mirrors `llvm/test/Verifier/absolute_symbol.ll` rejecting
/// `!absolute_symbol !{i64 0, i64 0}` as an empty range.
#[test]
fn absolute_symbol_zero_zero_is_empty_range() -> Result<(), IrError> {
    let m = module_new!("absolute_symbol_bad")?;
    let i8_ty = m.i8_type();
    let i64_ty = m.i64_type();
    let g = m.add_global("absolute_zero_zero", i8_ty.const_zero())?;
    let lo = m.metadata_constant(i64_ty.const_int(0_i64))?;
    let hi = m.metadata_constant(i64_ty.const_int(0_i64))?;
    let range = m.metadata_tuple([lo, hi])?;
    m.view(g)
        .set_metadata(&m, MetadataAttachmentKind::AbsoluteSymbol, range)?;

    let err = m
        .verify_borrowed()
        .expect_err("zero-zero absolute_symbol range must fail");
    assert!(matches!(
        err,
        IrError::VerifierFailure {
            rule: VerifierRule::RangeMetadataMalformed,
            ..
        }
    ));
    Ok(())
}
