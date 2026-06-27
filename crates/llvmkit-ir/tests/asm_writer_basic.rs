//! AsmWriter round-trip / format tests. Mirrors the early pieces of
//! `llvm/lib/IR/AsmWriter.cpp` exercised by the supported opcode set.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` cites `unittests/IR/AsmWriterTest.cpp` plus a
//! `test/Assembler/*.ll` fixture for the IR shape under test. The two
//! `unnamed_addr` assertions in `module_prints_simple_add_function` track
//! `test/Assembler/unnamed-addr.ll`.

use llvmkit_ir::{IRBuilder, IntValue, IrError, Linkage, Module};

/// Closest upstream coverage:
/// `unittests/IR/AsmWriterTest.cpp::TEST(AsmWriterTest, DebugPrintDetachedInstruction)`
/// (AsmWriter prints a function body with builder-emitted `add` and `ret`).
/// IR shape mirrors `test/Assembler/flags.ll` (basic add+ret rendering).
#[test]
fn module_prints_simple_add_function() -> Result<(), IrError> {
    Module::with_new("demo", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("add", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let lhs: IntValue<i32> = f.param(0)?.try_into()?;
        let rhs: IntValue<i32> = f.param(1)?.try_into()?;
        let sum = b.build_int_add(lhs, rhs, "sum")?;
        b.build_ret(sum)?;

        let text = format!("{m}");
        let expected = "; ModuleID = 'demo'\n\
            define i32 @add(i32 %0, i32 %1) {\n\
            entry:\n\
            \x20\x20%sum = add i32 %0, %1\n\
            \x20\x20ret i32 %sum\n\
            }\n";
        assert_eq!(text, expected, "got:\n{text}");
        // Default state has no `local_unnamed_addr` token.
        assert!(!text.contains("local_unnamed_addr"));
        assert!(!text.contains(" unnamed_addr"));
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/AsmWriter.cpp::printLLVMNameWithoutPrefix`: `$` is a
/// legal bare LLVM identifier character and must not force quotes.
#[test]
fn dollar_names_print_without_quotes() -> Result<(), IrError> {
    Module::with_new("dollar_names", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("foo$bar", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry$bb");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let arg: IntValue<i32> = f.param(0)?.try_into()?;
        let sum = b.build_int_add::<i32, _, _, _>(arg, 1_i32, "sum$value")?;
        b.build_ret(sum)?;

        let text = format!("{m}");
        assert!(text.contains("define i32 @foo$bar(i32 %0)"), "{text}");
        assert!(text.contains("entry$bb:"), "{text}");
        assert!(text.contains("%sum$value = add i32 %0, 1"), "{text}");
        Ok(())
    })
}
/// llvmkit-specific regression for LLVM's function-local `ValueSymbolTable`:
/// `Value.cpp::getSymTab` sends arguments, basic blocks, and instructions to
/// the same function symbol table, so they share one local namespace.
#[test]
fn function_local_names_share_argument_block_and_instruction_namespace() -> Result<(), IrError> {
    Module::with_new("local_names", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m
            .function_builder::<i32, _>("f", fn_ty)
            .param_name(0, "entry")
            .build()?;
        let entry = f.append_basic_block(&m, "entry");
        let entry_name = entry.name();
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let arg: IntValue<i32> = f.param(0)?.try_into()?;
        let result = b.build_int_add::<i32, _, _, _>(arg, 1_i32, "entry")?;
        b.build_ret(result)?;

        assert_eq!(f.param(0)?.name().as_deref(), Some("entry"));
        assert_eq!(entry_name.as_deref(), Some("entry1"));
        assert_eq!(result.name().as_deref(), Some("entry2"));

        let expected = "; ModuleID = 'local_names'\n\
            define i32 @f(i32 %entry) {\n\
            entry1:\n\
            \x20\x20%entry2 = add i32 %entry, 1\n\
            \x20\x20ret i32 %entry2\n\
            }\n";
        assert_eq!(format!("{m}"), expected);
        Ok(())
    })
}

/// llvmkit-specific regression for `Value::setNameImpl`: renaming a local value
/// creates a unique replacement before removing the old binding, then frees the
/// old name so a later value can reuse it. Closest upstream unit coverage:
/// `unittests/IR/ValueTest.cpp::TEST(ValueTest, setNameShrink)`.
#[test]
fn set_name_reinserts_and_frees_old_binding() -> Result<(), IrError> {
    Module::with_new("rename", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let arg: IntValue<i32> = f.param(0)?.try_into()?;

        let first = b.build_int_add::<i32, _, _, _>(arg, 1_i32, "tmp")?;
        let second = b.build_int_add::<i32, _, _, _>(first, 1_i32, "other")?;
        second.set_name(&m, "tmp");
        let third = b.build_int_add::<i32, _, _, _>(second, first, "other")?;
        b.build_ret(third)?;

        assert_eq!(second.name().as_deref(), Some("tmp1"));
        assert_eq!(third.name().as_deref(), Some("other"));
        let text = format!("{m}");
        assert!(text.contains("%tmp = add i32 %0, 1\n"), "{text}");
        assert!(text.contains("%tmp1 = add i32 %tmp, 1\n"), "{text}");
        assert!(text.contains("%other = add i32 %tmp1, %tmp\n"), "{text}");
        assert!(text.contains("ret i32 %other\n"), "{text}");
        Ok(())
    })
}

/// llvmkit-specific: exercises the IRBuilder constant-folder path -- both add
/// operands are constants so the folder elides the `add` and feeds `42`
/// directly to `ret`. Closest upstream coverage:
/// `unittests/IR/AsmWriterTest.cpp` (textual rendering of `ret i32 42`) and
/// `unittests/IR/ConstantsTest.cpp` (constant folding of integer arithmetic).
#[test]
fn module_prints_const_folded_arithmetic() -> Result<(), IrError> {
    Module::with_new("folded", |m| {
        // Two integer constants fed through the constant folder produce a
        // pre-folded ConstantInt operand for `ret`.
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<i32, _>("answer", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let a = i32_ty.const_int(40_i32);
        let bb = i32_ty.const_int(2_i32);
        // build_int_add on two constants: the folder produces a constant.
        // We feed it through build_int_add to exercise the fold path; the
        // folded value reaches the `ret` operand directly with no `add`
        // instruction emitted.
        let folded = b.build_int_add(
            IntValue::<i32>::try_from(a.as_value())?,
            IntValue::<i32>::try_from(bb.as_value())?,
            "sum",
        )?;
        b.build_ret(folded)?;

        let text = format!("{m}");
        // The folded value is a constant; it should print as `42`.
        assert!(text.contains("ret i32 42\n"), "got:\n{text}");
        Ok(())
    })
}

/// llvmkit-specific: `Display` on `Function` matches the function section
/// produced by `Display` on `Module`. Closest upstream coverage:
/// `unittests/IR/AsmWriterTest.cpp::TEST(AsmWriterTest, DebugPrintDetachedInstruction)`
/// (uses `Function::print` independently of `Module::print`).
#[test]
fn function_print_standalone_matches_module_section() -> Result<(), IrError> {
    Module::with_new("standalone", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("identity", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let arg: IntValue<i32> = f.param(0)?.try_into()?;
        b.build_ret(arg)?;

        let standalone = format!("{f}");
        let module = format!("{m}");
        assert!(module.contains(&standalone), "module did not include f");
        Ok(())
    })
}

/// llvmkit-specific: a function with no body (no appended basic blocks) prints
/// as `declare`. Closest upstream coverage:
/// `unittests/IR/AsmWriterTest.cpp` (textual rendering paths) and
/// `lib/IR/AsmWriter.cpp::AssemblyWriter::printFunction` declare-vs-define
/// branch.
#[test]
fn declare_form_for_empty_function() -> Result<(), IrError> {
    Module::with_new("declare_only", |m| {
        let void = m.void_type();
        let fn_ty = m.fn_type(void.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
        let _ = m.add_function::<(), _>("ext", fn_ty, Linkage::External)?;
        let text = format!("{m}");
        assert!(text.contains("declare void @ext()\n"), "got:\n{text}");
        Ok(())
    })
}

/// Mirrors `test/Assembler/numbered-values.ll` (slot numbering for unnamed
/// values and basic blocks). Closest unit-test coverage:
/// `unittests/IR/AsmWriterTest.cpp::TEST(AsmWriterTest, DebugPrintDetachedArgument)`
/// (slot-numbered argument rendering).
#[test]
fn unnamed_basic_block_uses_slot_label() -> Result<(), IrError> {
    Module::with_new("slots", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("anon", fn_ty, Linkage::External)?;
        // No name on the entry block.
        let entry = f.append_basic_block(&m, "");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let arg: IntValue<i32> = f.param(0)?.try_into()?;
        b.build_ret(arg)?;
        let text = format!("{m}");
        // Block 0 (the only block) should label as `1:` because slot 0 is
        // claimed by the unnamed argument %0.
        assert!(
            text.contains("1:\n"),
            "expected slot-labelled block; got:\n{text}"
        );
        Ok(())
    })
}

/// Mirrors `llvm/lib/IR/Module.cpp::Module::setSourceFileName` and
/// `llvm/lib/IR/AsmWriter.cpp::AssemblyWriter::printModule`: source filename
/// is stored by the module, exposed as a borrowed string view, and omitted
/// again after clearing.
#[test]
fn source_filename_api_borrows_and_clears() {
    Module::with_new("source_filename_api", |m| {
        assert!(m.source_filename().is_none());
        m.set_source_filename("dir/file.c");

        let borrowed: core::cell::Ref<'_, str> = m.source_filename().expect("source filename");
        assert_eq!(&*borrowed, "dir/file.c");
        assert_eq!(
            format!("{m}"),
            "; ModuleID = 'source_filename_api'\nsource_filename = \"dir/file.c\"\n"
        );
        drop(borrowed);
        m.clear_source_filename();
        assert!(m.source_filename().is_none());
        assert_eq!(format!("{m}"), "; ModuleID = 'source_filename_api'\n");
    })
}
