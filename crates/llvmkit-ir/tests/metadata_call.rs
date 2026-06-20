//! `call` instructions with `metadata` arguments and post-hoc function
//! attributes.
//!
//! Exercises the `MetadataAsValue` bridge (mirrors LLVM's
//! `MetadataAsValue`) that lets a metadata node be passed as a `call`
//! argument — the shape the named-register intrinsics
//! `@llvm.read_register` / `@llvm.write_register` require — plus the
//! `FunctionValue::add_attribute` setter for forward-declared functions.

use llvmkit_ir::{
    AttrIndex, AttrKind, Attribute, IRBuilder, IrError, Linkage, MetadataRef, Module,
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
        let void_ty = m.void_type();
        let md_ty = m.metadata_type();

        // !N = !{!"rsp"}  — a tuple whose only operand is the register name.
        let s = m.metadata_string("rsp");
        let node = m.metadata_tuple([MetadataRef(s)]);
        let md = m.metadata_as_value(node);

        // declare i64  @llvm.read_register.i64(metadata)
        let read_ty = m.fn_type(i64_ty, [md_ty.as_type()], false);
        let read = m.add_function::<i64>("llvm.read_register.i64", read_ty, Linkage::External)?;
        // declare void @llvm.write_register.i64(metadata, i64)
        let write_ty = m.fn_type(
            void_ty.as_type(),
            [md_ty.as_type(), i64_ty.as_type()],
            false,
        );
        let write = m.add_function::<()>("llvm.write_register.i64", write_ty, Linkage::External)?;

        // define i64 @get_sp() { %rsp = call ...; call void ...; ret i64 %rsp }
        let host_ty = m.fn_type(i64_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let host = m.add_function::<i64>("get_sp", host_ty, Linkage::External)?;
        let entry = host.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);

        let rsp = b.build_call(read, [md], "rsp")?;
        let rsp_val = rsp.return_int_value();
        b.build_call(write, [md, rsp_val.as_value()], "")?;
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

        // Forward declaration via plain `add_function` (no builder).
        let f = m.add_function::<()>("trampoline", fn_ty, Linkage::External)?;
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
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        b.build_ret_void();

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
        let g = m.add_function::<()>("g", fn_ty, Linkage::External)?;
        let host_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<()>("f", host_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let s = m.metadata_string("rsp");
        let md = m.metadata_as_value(s);
        b.build_call(g, [md], "")?;
        b.build_ret_void();

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
