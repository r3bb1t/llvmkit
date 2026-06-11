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

/// Build the read/write named-register intrinsics, emit calls whose
/// argument is a `metadata` node, and assert the printed module carries
/// the `metadata !N` operands plus the `!{` node defining the register
/// string. Mirrors `AsmWriter.cpp::writeAsOperandInternal(Value*)` for
/// `MetadataAsValue` call operands.
#[test]
fn call_with_metadata_argument() -> Result<(), IrError> {
    let m = Module::new("named_registers");
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
    let entry = host.append_basic_block("entry");
    let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);

    let rsp = b.build_call(read, [md], "rsp")?;
    let rsp_val = rsp.return_int_value();
    b.build_call(write, [md, rsp_val.as_value()], "")?;
    b.build_ret(rsp_val)?;

    let text = format!("{m}");
    assert!(
        text.contains("@llvm.read_register.i64(metadata !"),
        "output:\n{text}"
    );
    assert!(
        text.contains("@llvm.write_register.i64(metadata !"),
        "output:\n{text}"
    );
    // The rsp string is defined inline in a `!{ ... }` tuple node.
    assert!(text.contains(r#"!{!"rsp"}"#), "output:\n{text}");
    Ok(())
}

/// A forward-declared function gains attributes after creation via
/// `FunctionValue::add_attribute` / `set_string_attribute`, and they
/// print on the definition. Mirrors `Function::addFnAttr` usage where a
/// declaration is created first and decorated as its body is emitted.
#[test]
fn post_construction_function_attributes() -> Result<(), IrError> {
    let m = Module::new("attrs");
    let void_ty = m.void_type();
    let fn_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);

    // Forward declaration via plain `add_function` (no builder).
    let f = m.add_function::<()>("trampoline", fn_ty, Linkage::External)?;
    // Body is defined later; decorate the existing value.
    f.add_attribute(
        AttrIndex::Function,
        Attribute::enum_attr(AttrKind::NoRedZone).expect("flag attribute"),
    );
    f.add_attribute(
        AttrIndex::Function,
        Attribute::enum_attr(AttrKind::Naked).expect("flag attribute"),
    );
    f.set_string_attribute(AttrIndex::Function, "frame-pointer", "all");

    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    b.build_ret_void();

    let text = format!("{m}");
    assert!(text.contains("noredzone"), "output:\n{text}");
    assert!(text.contains("naked"), "output:\n{text}");
    assert!(text.contains(r#""frame-pointer"="all""#), "output:\n{text}");
    Ok(())
}

/// `metadata_as_value` uniques by metadata node (mirrors LLVM's
/// `MetadataAsValue::get`): wrapping the same node twice yields the
/// identical `Value`, so value identity/equality stays meaningful and
/// the use-list is not fragmented across duplicates.
#[test]
fn metadata_as_value_is_uniqued() {
    let m = Module::new("u");
    let s = m.metadata_string("rsp");
    let node = m.metadata_tuple([MetadataRef(s)]);
    let a = m.metadata_as_value(node);
    let b = m.metadata_as_value(node);
    assert_eq!(a, b, "same metadata node must yield the same Value");
}

/// A bare MDString used through `MetadataAsValue` prints inline as `!"rsp"`,
/// not as a numbered top-level `!N = !"rsp"` definition.
/// Mirrors `AsmWriter.cpp::writeAsOperandInternal(Metadata*)` MDString arm.
#[test]
fn metadata_string_as_value_prints_inline() -> Result<(), IrError> {
    let m = Module::new("md_string_value");
    let void_ty = m.void_type();
    let md_ty = m.metadata_type();
    let fn_ty = m.fn_type(void_ty.as_type(), [md_ty.as_type()], false);
    let g = m.add_function::<()>("g", fn_ty, Linkage::External)?;
    let host_ty = m.fn_type(void_ty.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m.add_function::<()>("f", host_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
    let s = m.metadata_string("rsp");
    let md = m.metadata_as_value(s);
    b.build_call(g, [md], "")?;
    b.build_ret_void();

    let text = format!("{m}");
    assert!(
        text.contains(r#"call void @g(metadata !"rsp")"#),
        "output:\n{text}"
    );
    assert!(
        !text.contains(r#" = !"rsp""#),
        "MDString must not be top-level numbered:\n{text}"
    );
    Ok(())
}

/// Named metadata operands are MDNode references; MDStrings inside those nodes
/// still print inline in the referenced tuple body.
/// Mirrors `AsmWriter.cpp::writeAllMDNodes` emitting only MDNodes while
/// `AsmWriter.cpp::writeAsOperandInternal(Metadata*)` prints MDStrings inline.
#[test]
fn string_referenced_by_named_metadata_is_not_dangling() {
    let m = Module::new("d");
    let s = m.metadata_string("x");
    let tuple = m.metadata_tuple([MetadataRef(s)]);
    let idx = m.get_or_insert_named_metadata("my.named");
    m.named_metadata_add_operand(idx, MetadataRef(tuple));

    let text = format!("{m}");
    assert!(text.contains(r#"!0 = !{!"x"}"#), "output:\n{text}");
    assert!(text.contains("!my.named = !{!0}"), "output:\n{text}");
}
