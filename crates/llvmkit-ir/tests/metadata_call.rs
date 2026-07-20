//! `call` instructions with `metadata` arguments and post-hoc function
//! attributes.
//!
//! Exercises the `MetadataAsValue` bridge (mirrors LLVM's
//! `MetadataAsValue`) that lets a metadata node be passed as a `call`
//! argument — the shape the named-register intrinsics
//! `@llvm.read_register` / `@llvm.write_register` require — plus the
//! `FunctionValue::add_attribute` setter for forward-declared functions.

use llvmkit_ir::{
    AttrIndex, AttrKind, Attribute, Dyn, IRBuilder, InstructionView, IrError, Linkage,
    MetadataAttachmentKind, MetadataRef, Module, NoFolder, Ptr, VerifierRule,
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
    Module::with_new("named_registers", |m| {
        let i64_ty = m.i64_type();

        // !N = !{!"rsp"}  — a tuple whose only operand is the register name.
        let s = m.metadata_string("rsp");
        let node = m.metadata_tuple([MetadataRef(s)]);
        let md = m.metadata_as_value(node);

        // declare i64  @llvm.read_register.i64(metadata)
        let read = m.get_or_insert_intrinsic_declaration_by_name("llvm.read_register.i64")?;
        // declare void @llvm.write_register.i64(metadata, i64)
        let write = m.get_or_insert_intrinsic_declaration_by_name("llvm.write_register.i64")?;

        // define i64 @get_sp() { %rsp = call ...; call void ...; ret i64 %rsp }
        let host_ty = m.fn_type(i64_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let host = m.add_function_dyn("get_sp", host_ty, Linkage::External)?;
        let entry = host.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);

        let rsp = b.build_call_dyn(read, [md], "rsp")?;
        let rsp_val: llvmkit_ir::IntValue<i64> = rsp
            .return_value()
            .expect("read_register returns value")
            .try_into()?;
        b.build_call_dyn(write, [md, rsp_val.as_value()], "")?;
        b.build_ret(rsp_val)?;

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
        let write_line =
            write_line.unwrap_or_else(|| panic!("missing write-register call:\n{text}"));
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
    })
}

/// A forward-declared function gains attributes after creation via
/// `FunctionValue::add_attribute` / `set_string_attribute`, and they
/// print on the definition. Mirrors `Function::addFnAttr` usage where a
/// declaration is created first and decorated as its body is emitted.
#[test]
fn post_construction_function_attributes() -> Result<(), IrError> {
    Module::with_new("attrs", |m| {
        let void_ty = m.void_type();
        let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);

        // Forward declaration via `add_function_dyn` (no builder).
        let f = m.add_function_dyn("trampoline", fn_ty, Linkage::External)?;
        // Body is defined later; decorate the existing value.
        f.add_attribute(
            &m,
            AttrIndex::Function,
            Attribute::enum_attr(AttrKind::NoRedZone).expect("flag attribute"),
        );
        f.add_attribute(
            &m,
            AttrIndex::Function,
            Attribute::enum_attr(AttrKind::Naked).expect("flag attribute"),
        );
        f.set_string_attribute(&m, AttrIndex::Function, "frame-pointer", "all");

        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        b.build_ret_void()?;

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
    })
}

/// `metadata_as_value` uniques by metadata node (mirrors LLVM's
/// `MetadataAsValue::get`): wrapping the same node twice yields the
/// identical `Value`, so value identity/equality stays meaningful and
/// the use-list is not fragmented across duplicates.
#[test]
fn metadata_as_value_is_uniqued() {
    Module::with_new("u", |m| {
        let s = m.metadata_string("rsp");
        let node = m.metadata_tuple([MetadataRef(s)]);
        let a = m.metadata_as_value(node);
        let b = m.metadata_as_value(node);
        assert_eq!(a, b, "same metadata node must yield the same Value");
    })
}

/// A bare MDString used through `MetadataAsValue` prints inline as `!"rsp"`,
/// not as a numbered top-level `!N = !"rsp"` definition.
/// Mirrors `AsmWriter.cpp::writeAsOperandInternal(Metadata*)` MDString arm.
#[test]
fn metadata_string_as_value_prints_inline() -> Result<(), IrError> {
    Module::with_new("md_string_value", |m| {
        let void_ty = m.void_type();
        let md_ty = m.metadata_type();
        let fn_ty = m.fn_type(void_ty.as_type(), [md_ty.as_type()], false);
        let g = m.add_function_dyn("g", fn_ty, Linkage::External)?;
        let host_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function_dyn("f", host_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let s = m.metadata_string("rsp");
        let md = m.metadata_as_value(s);
        b.build_call_dyn(g, [md], "")?;
        b.build_ret_void()?;

        let text = format!("{m}");
        assert_line(&text, r#"  call void @g(metadata !"rsp")"#);
        assert_no_line_with_fragment(&text, r#" = !"rsp""#);
        Ok(())
    })
}

/// Named metadata operands are MDNode references; MDStrings inside those nodes
/// still print inline in the referenced tuple body.
/// Mirrors `AsmWriter.cpp::writeAllMDNodes` emitting only MDNodes while
/// `AsmWriter.cpp::writeAsOperandInternal(Metadata*)` prints MDStrings inline.
#[test]
fn string_referenced_by_named_metadata_is_not_dangling() {
    Module::with_new("d", |m| {
        let s = m.metadata_string("x");
        let tuple = m.metadata_tuple([MetadataRef(s)]);
        let idx = m.get_or_insert_named_metadata("my.named");
        m.named_metadata_add_operand(idx, MetadataRef(tuple));

        let text = format!("{m}");
        assert_line(&text, r#"!0 = !{!"x"}"#);
        assert_line(&text, "!my.named = !{!0}");
    })
}

/// Mirrors `llvm/test/Analysis/ValueTracking/known-bits-from-range-md.ll`
/// and `llvm/test/Verifier/absolute_symbol.ll` typed integer metadata operands.
#[test]
fn metadata_constant_tuple_prints_typed_constants() {
    Module::with_new("mdc", |m| {
        let i64_ty = m.i64_type();
        let one = m.metadata_constant(i64_ty.const_int(1_i64));
        let five = m.metadata_constant(i64_ty.const_int(5_i64));
        let tuple = m.metadata_tuple([MetadataRef(one), MetadataRef(five)]);
        let idx = m.get_or_insert_named_metadata("ranges");
        m.named_metadata_add_operand(idx, MetadataRef(tuple));

        let text = format!("{m}");
        assert_line(&text, "!0 = !{i64 1, i64 5}");
        assert_line(&text, "!ranges = !{!0}");
    })
}

/// Mirrors `Verifier::visitRangeMetadata` accepting well-formed `!range`
/// metadata on loads.
#[test]
fn range_metadata_on_load_verifies_and_prints() -> Result<(), IrError> {
    Module::with_new("range_ok", |m| {
        let i8_ty = m.i8_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(i8_ty, [ptr_ty.as_type()], false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let p: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
        let ld = b.build_int_load::<i8, _, _>(p, "v")?;
        let lo = m.metadata_constant(i8_ty.const_int(0x10_u8));
        let hi = m.metadata_constant(i8_ty.const_int(0x20_u8));
        let range = m.metadata_tuple([MetadataRef(lo), MetadataRef(hi)]);
        let inst = InstructionView::try_from(ld.as_value())?;
        inst.set_metadata(MetadataAttachmentKind::Range, range);
        b.build_ret(ld)?;

        m.verify_borrowed()?;
        let text = format!("{m}");
        assert_line(&text, "  %v = load i8, ptr %0, align 1, !range !0");
        assert_line(&text, "!0 = !{i8 16, i8 32}");
        Ok(())
    })
}

/// Mirrors `Verifier::verifyRangeLikeMetadata` rejecting an unfinished
/// range operand list.
#[test]
fn range_metadata_rejects_odd_operand_count() -> Result<(), IrError> {
    Module::with_new("range_odd", |m| {
        let i8_ty = m.i8_type();
        let ptr_ty = m.ptr_type(0);
        let fn_ty = m.fn_type(i8_ty, [ptr_ty.as_type()], false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(entry);
        let p: llvmkit_ir::PointerValue = f.param(0)?.try_into()?;
        let ld = b.build_int_load::<i8, _, _>(p, "v")?;
        let lo = m.metadata_constant(i8_ty.const_int(0x10_u8));
        let range = m.metadata_tuple([MetadataRef(lo)]);
        let inst = InstructionView::try_from(ld.as_value())?;
        inst.set_metadata(MetadataAttachmentKind::Range, range);
        b.build_ret(ld)?;

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
    })
}

/// Mirrors `llvm/test/Verifier/range-2.ll` allowing `!range` on call and
/// invoke return values.
#[test]
fn range_metadata_on_call_and_invoke_verifies() -> Result<(), IrError> {
    Module::with_new("range_call_invoke_ok", |m| {
        let i8_ty = m.i8_type();
        let ptr_ty = m.ptr_type(0);
        let callee = m
            .add_typed_function::<i8, (Ptr,), _>("callee", Linkage::External)?
            .as_function();
        let lo = m.metadata_constant(i8_ty.const_int(0_i8));
        let hi = m.metadata_constant(i8_ty.const_int(1_i8));
        let range = m.metadata_tuple([MetadataRef(lo), MetadataRef(hi)]);

        let call_host_ty = m.fn_type(i8_ty, [ptr_ty.as_type()], false);
        let call_host = m.add_function_dyn("call_host", call_host_ty, Linkage::External)?;
        let call_entry = call_host.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<Dyn>(&m).position_at_end(call_entry);
        let p: llvmkit_ir::PointerValue = call_host.param(0)?.try_into()?;
        let call = b.build_call_dyn(callee, [p.as_value()], "v")?;
        call.as_view()
            .set_metadata(MetadataAttachmentKind::Range, range);
        b.build_ret(call.return_int_value())?;

        let invoke_host_ty = m.fn_type(i8_ty, [ptr_ty.as_type()], false);
        let invoke_host = m.add_function_dyn("invoke_host", invoke_host_ty, Linkage::External)?;
        let entry = invoke_host.append_basic_block(&m, "entry");
        let normal = invoke_host.append_basic_block(&m, "normal");
        let unwind = invoke_host.append_basic_block(&m, "unwind");
        let normal_label = normal.label();
        let unwind_label = unwind.label();
        let p: llvmkit_ir::PointerValue = invoke_host.param(0)?.try_into()?;
        let (_entry, invoke) = IRBuilder::new_for::<Dyn>(&m)
            .position_at_end(entry)
            .build_invoke_dyn(callee, [p.as_value()], normal_label, unwind_label, "v")?;
        invoke
            .as_view()
            .set_metadata(MetadataAttachmentKind::Range, range);
        let invoke_value: llvmkit_ir::IntValue<i8> = invoke.as_value().try_into()?;
        IRBuilder::new_for::<Dyn>(&m)
            .position_at_end(normal)
            .build_ret(invoke_value)?;
        IRBuilder::new_for::<Dyn>(&m)
            .position_at_end(unwind)
            .build_ret(i8_ty.const_zero())?;

        m.verify_borrowed()
    })
}

/// Mirrors `Verifier::visitInstruction` rejecting `!range` on non
/// load/call/invoke instructions.
#[test]
fn range_metadata_rejects_non_load_call_invoke_user() -> Result<(), IrError> {
    Module::with_new("range_bad_user", |m| {
        let i8_ty = m.i8_type();
        let fn_ty = m.fn_type(i8_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let add =
            b.build_int_add::<i8, _, _, _>(i8_ty.const_int(1_u8), i8_ty.const_int(2_u8), "sum")?;
        let lo = m.metadata_constant(i8_ty.const_int(0x10_u8));
        let hi = m.metadata_constant(i8_ty.const_int(0x20_u8));
        let range = m.metadata_tuple([MetadataRef(lo), MetadataRef(hi)]);
        let inst = InstructionView::try_from(add.as_value())?;
        inst.set_metadata(MetadataAttachmentKind::Range, range);
        b.build_ret(add)?;

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
    })
}

/// Mirrors `llvm/test/Verifier/absolute_symbol.ll` rejecting
/// `!absolute_symbol !{i64 0, i64 0}` as an empty range.
#[test]
fn absolute_symbol_zero_zero_is_empty_range() -> Result<(), IrError> {
    Module::with_new("absolute_symbol_bad", |m| {
        let i8_ty = m.i8_type();
        let i64_ty = m.i64_type();
        let g = m.add_global("absolute_zero_zero", i8_ty.const_zero())?;
        let lo = m.metadata_constant(i64_ty.const_int(0_i64));
        let hi = m.metadata_constant(i64_ty.const_int(0_i64));
        let range = m.metadata_tuple([MetadataRef(lo), MetadataRef(hi)]);
        g.set_metadata(&m, MetadataAttachmentKind::AbsoluteSymbol, range);

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
    })
}
