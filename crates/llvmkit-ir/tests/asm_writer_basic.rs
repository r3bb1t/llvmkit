//! AsmWriter round-trip / format tests. Mirrors the early pieces of
//! `llvm/lib/IR/AsmWriter.cpp` exercised by the supported opcode set.
//!
//! ## Upstream provenance
//!
//! Each `#[test]` cites `unittests/IR/AsmWriterTest.cpp` plus a
//! `test/Assembler/*.ll` fixture for the IR shape under test. The two
//! `unnamed_addr` assertions in `module_prints_simple_add_function` track
//! `test/Assembler/unnamed-addr.ll`.

use llvmkit_ir::{Dyn, IntValue, IrBuilder, IrError, Linkage, Type, module_new};

/// Closest upstream coverage:
/// `unittests/IR/AsmWriterTest.cpp::TEST(AsmWriterTest, DebugPrintDetachedInstruction)`
/// (AsmWriter prints a function body with builder-emitted `add` and `ret`).
/// IR shape mirrors `test/Assembler/flags.ll` (basic add+ret rendering).
#[test]
fn module_prints_simple_add_function() -> Result<(), IrError> {
    let m = module_new!("demo")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()]);
    let f = m.add_function_dyn("add", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let lhs: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let rhs: IntValue<'_, i32, _> = m.view(f).param(1)?.try_into()?;
    let sum = b.int_add(lhs, rhs, "sum")?;
    b.ret(sum)?;

    let text = format!("{m}");
    let expected = "; ModuleID = 'demo'\n\
        \n\
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
}

/// Mirrors `llvm/lib/IR/AsmWriter.cpp::AssemblyWriter::printModule`: after
/// `printTypeIdentities()` emits named struct identities, the function loop
/// writes a blank line before `printFunction`.
#[test]
fn module_prints_blank_line_between_type_identities_and_first_function() -> Result<(), IrError> {
    let m = module_new!("type_separator")?;
    let i32_ty = m.i32_type();
    let point_ty = m.get_or_insert_named_struct("Point");
    m.set_struct_body_dyn(point_ty, [i32_ty.as_type(), i32_ty.as_type()], false)?;

    let fn_ty = m.function_type(m.void_type(), [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(entry)
        .ret_void()?;

    let expected = "; ModuleID = 'type_separator'\n\
        \n\
        %Point = type { i32, i32 }\n\
        \n\
        define void @f(i32 %0) {\n\
        entry:\n\
        \x20\x20ret void\n\
        }\n";
    let text = format!("{m}");
    assert_eq!(text, expected, "got:\n{text}");
    Ok(())
}

/// Mirrors `llvm::printLLVMNameWithoutPrefix` (`lib/IR/AsmWriter.cpp`): the
/// unquoted set is `isalnum(C) || C == '-' || C == '.' || C == '_'`, and `$` is
/// outside it, so every `$`-bearing name is quoted on output — a function, a
/// block label and an instruction result alike, since all three go through the
/// one routine. `LLLexer` *accepts* a bare `$` on input, which is why
/// `test/Assembler/block-labels.ll` writes `br label %$N` and CHECKs for
/// `br label %"$N"`; the asymmetry is upstream's, and this test pins the API
/// side of it, where no fixture can reach.
///
/// This test previously asserted the opposite, on the claim that `$` "is a
/// legal bare LLVM identifier character and must not force quotes" — which was
/// the `$`-quoting divergence, encoded as its own expectation.
#[test]
fn dollar_names_print_quoted() -> Result<(), IrError> {
    let m = module_new!("dollar_names")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("foo$bar", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry$bb");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let arg: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let sum = b.int_add::<i32, _, _, _>(arg, 1_i32, "sum$value")?;
    b.ret(sum)?;

    let text = format!("{m}");
    assert!(text.contains("define i32 @\"foo$bar\"(i32 %0)"), "{text}");
    assert!(text.contains("\"entry$bb\":"), "{text}");
    assert!(text.contains("%\"sum$value\" = add i32 %0, 1"), "{text}");
    Ok(())
}
/// llvmkit-specific regression for LLVM's function-local `ValueSymbolTable`:
/// `Value.cpp::getSymTab` sends arguments, basic blocks, and instructions to
/// the same function symbol table, so they share one local namespace.
#[test]
fn function_local_names_share_argument_block_and_instruction_namespace() -> Result<(), IrError> {
    let m = module_new!("local_names")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m
        .function_builder::<i32, _>("f", fn_ty)
        .param_name(0, "entry")
        .build()?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let entry_name = entry.name();
    let b = IrBuilder::new_for::<i32>(&m).position_at_end(entry);
    let arg: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let result = b.int_add::<i32, _, _, _>(arg, 1_i32, "entry")?;
    b.ret(result)?;

    assert_eq!(m.view(f).param(0)?.name().as_deref(), Some("entry"));
    assert_eq!(entry_name.as_deref(), Some("entry1"));
    assert_eq!(m.view(result).name().as_deref(), Some("entry2"));

    let expected = "; ModuleID = 'local_names'\n\
        \n\
        define i32 @f(i32 %entry) {\n\
        entry1:\n\
        \x20\x20%entry2 = add i32 %entry, 1\n\
        \x20\x20ret i32 %entry2\n\
        }\n";
    assert_eq!(format!("{m}"), expected);
    Ok(())
}

/// llvmkit-specific regression for `Value::setNameImpl`: renaming a local value
/// creates a unique replacement before removing the old binding, then frees the
/// old name so a later value can reuse it. Closest upstream unit coverage:
/// `unittests/IR/ValueTest.cpp::TEST(ValueTest, setNameShrink)`.
#[test]
fn set_name_reinserts_and_frees_old_binding() -> Result<(), IrError> {
    let m = module_new!("rename")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let arg: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;

    let first = b.int_add::<i32, _, _, _>(arg, 1_i32, "tmp")?;
    let second = b.int_add::<i32, _, _, _>(first, 1_i32, "other")?;
    b.view(second).set_name(&m, "tmp");
    let third = b.int_add::<i32, _, _, _>(second, first, "other")?;
    b.ret(third)?;

    assert_eq!(m.view(second).name().as_deref(), Some("tmp1"));
    assert_eq!(m.view(third).name().as_deref(), Some("other"));
    let text = format!("{m}");
    assert!(text.contains("%tmp = add i32 %0, 1\n"), "{text}");
    assert!(text.contains("%tmp1 = add i32 %tmp, 1\n"), "{text}");
    assert!(text.contains("%other = add i32 %tmp1, %tmp\n"), "{text}");
    assert!(text.contains("ret i32 %other\n"), "{text}");
    Ok(())
}

/// llvmkit-specific: exercises the IrBuilder constant-folder path -- both add
/// operands are constants so the folder elides the `add` and feeds `42`
/// directly to `ret`. Closest upstream coverage:
/// `unittests/IR/AsmWriterTest.cpp` (textual rendering of `ret i32 42`) and
/// `unittests/IR/ConstantsTest.cpp` (constant folding of integer arithmetic).
#[test]
fn module_prints_const_folded_arithmetic() -> Result<(), IrError> {
    let m = module_new!("folded")?;
    // Two integer constants fed through the constant folder produce a
    // pre-folded ConstantInt operand for `ret`.
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, Vec::<llvmkit_ir::Type<'_, _>>::new());
    let f = m.add_function_dyn("answer", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let a = i32_ty.const_int(40_i32);
    let bb = i32_ty.const_int(2_i32);
    // int_add on two constants: the folder produces a constant.
    // We feed it through int_add to exercise the fold path; the
    // folded value reaches the `ret` operand directly with no `add`
    // instruction emitted.
    let folded = b.int_add(
        IntValue::<i32, _>::try_from(a.as_erased())?,
        IntValue::<i32, _>::try_from(bb.as_erased())?,
        "sum",
    )?;
    b.ret(folded)?;

    let text = format!("{m}");
    // The folded value is a constant; it should print as `42`.
    assert!(text.contains("ret i32 42\n"), "got:\n{text}");
    Ok(())
}

/// llvmkit-specific: `Display` on `Function` matches the function section
/// produced by `Display` on `Module`. Closest upstream coverage:
/// `unittests/IR/AsmWriterTest.cpp::TEST(AsmWriterTest, DebugPrintDetachedInstruction)`
/// (uses `Function::print` independently of `Module::print`).
#[test]
fn function_print_standalone_matches_module_section() -> Result<(), IrError> {
    let m = module_new!("standalone")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("identity", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");

    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let arg: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    b.ret(arg)?;

    let standalone = format!("{}", m.view(f));
    let module = format!("{m}");
    assert!(module.contains(&standalone), "module did not include f");
    Ok(())
}

/// llvmkit-specific: a function with no body (no appended basic blocks) prints
/// as `declare`. Closest upstream coverage:
/// `unittests/IR/AsmWriterTest.cpp` (textual rendering paths) and
/// `lib/IR/AsmWriter.cpp::AssemblyWriter::printFunction` declare-vs-define
/// branch.
#[test]
fn declare_form_for_empty_function() -> Result<(), IrError> {
    let m = module_new!("declare_only")?;
    let void = m.void_type();
    let fn_ty = m.function_type(void.as_type(), Vec::<llvmkit_ir::Type<'_, _>>::new());
    let _ = m.add_function_dyn("ext", fn_ty, Linkage::External)?;
    let text = format!("{m}");
    assert!(text.contains("declare void @ext()\n"), "got:\n{text}");
    Ok(())
}

/// **No upstream `.ll` counterpart:** this hand-builds `@anon(i32 %0)` rather
/// than parsing a fixture, so it is registered `llvmkit-specific` rather than
/// `mirror`. The rules it locks are upstream's, cited by symbol:
/// `llvm/lib/IR/AsmWriter.cpp::AssemblyWriter::printBasicBlock`'s
/// `else if (!IsEntryBlock)` slot-label branch — an unnamed **entry** block
/// prints no label at all yet still holds its slot, and a later unnamed block
/// prints that slot and names the entry's in its predecessors comment — and
/// `llvm/lib/IR/AsmWriter.cpp::SlotTracker::processFunction`, which numbers
/// unnamed arguments before basic blocks.
///
/// The arg-before-block slot order has a genuine FileCheck oracle upstream:
/// `test/Assembler/block-labels.ll::@test2`'s
/// `; CHECK-LABEL: define void @test2(i32 %0, i32 %1) {` followed by
/// `; CHECK-NEXT:    ret void`, which is asserted against the vendored fixture
/// by `crates/llvmkit-asmparser/tests/parser_function_body.rs::an_unnamed_entry_block_prints_no_label`.
/// That fixture's `@test1` cannot adjudicate the assertions below: its `%X` is
/// a *named* argument, so its `; CHECK: 2: ; preds = %0` reads `%0` where this
/// module's unnamed argument pushes the entry block to slot 1.
/// `unittests/IR/AsmWriterTest.cpp::TEST(AsmWriterTest, DebugPrintDetachedArgument)`
/// is the closest unit test but covers the opposite condition — a *detached*
/// argument, which prints `i32 <badref>`.
#[test]
fn unnamed_basic_block_uses_slot_label() -> Result<(), IrError> {
    let m = module_new!("slots")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("anon", fn_ty, Linkage::External)?;
    // No name on either block.
    let entry = m.view(f).append_basic_block(&m, "");
    let tail = m.view(f).append_basic_block(&m, "");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    b.br(&tail)?;
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(tail);
    let arg: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    b.ret(arg)?;
    let text = format!("{m}");
    // Slot 0 is claimed by the unnamed argument `%0`, so the entry block is
    // slot 1 and the tail block slot 2.
    assert!(
        text.contains("define i32 @anon(i32 %0) {\n  br label %2\n"),
        "the unnamed entry block prints no label; got:\n{text}"
    );
    assert!(
        text.contains("2:                                                ; preds = %1\n"),
        "expected slot-labelled block; got:\n{text}"
    );
    Ok(())
}

/// Mirrors `llvm/lib/IR/Module.cpp::Module::setSourceFileName` and
/// `llvm/lib/IR/AsmWriter.cpp::AssemblyWriter::printModule`: source filename
/// is stored by the module, exposed as a borrowed string view, and omitted
/// again after clearing.
#[test]
fn source_filename_api_borrows_and_clears() {
    let m = module_new!("source_filename_api").expect("fresh module");
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
}

/// Mirrors `AssemblyWriter::printModule`'s function loop, which is
/// `for (const Function &F : *M) { Out << '\n'; printFunction(&F); }` — the
/// blank line is **unconditional**. llvmkit guarded it on the module also
/// having globals, aliases, ifuncs or named structs, so every module without
/// one of those printed one byte short of `llvm-dis`, and the shortfall was on
/// the first function only.
///
/// **Anchored on the routine, not on a fixture**: FileCheck cannot pin a blank
/// line, so no upstream `CHECK` block asserts this. The corroborating artefact
/// is `test/Assembler/debug-label-bitcode.ll`, whose checked-in body is
/// `llvm-as | llvm-dis` output and reads `source_filename = "…"`, a blank
/// line, then `; Function Attrs: …` — with no global, alias, ifunc or named
/// struct anywhere in that module.
#[test]
fn module_prints_a_blank_line_before_every_function_including_the_first() -> Result<(), IrError> {
    let m = module_new!("blank_lines")?;
    let fn_ty = m.function_type(m.void_type(), Vec::<Type<'_, _>>::new());
    m.add_function_dyn("a", fn_ty, Linkage::External)?;
    m.add_function_dyn("b", fn_ty, Linkage::External)?;

    let text = format!("{m}");
    let expected = "; ModuleID = 'blank_lines'\n\
        \n\
        declare void @a()\n\
        \n\
        declare void @b()\n";
    assert_eq!(text, expected, "got:\n{text}");
    Ok(())
}

/// Ports `unittests/IR/AsmWriterTest.cpp::TEST(AsmWriterTest,
/// DebugPrintDetachedArgument)`, whose whole assertion is
/// `EXPECT_EQ(S, "i32 <badref>")`.
///
/// `writeAsOperandInternal`'s value path ends
/// `if (Slot != -1) Out << Prefix << Slot; else Out << "<badref>";` — the
/// failure spelling carries no sigil in either the `'%'` or the `'@'` branch,
/// and llvmkit spelled it `%<unnumbered>` / `@<unnumbered>`.
///
/// **Input substitution, recorded rather than routed around.** Upstream builds
/// `new Argument(Ty)` with no parent; llvmkit has no detached IR — an
/// `Argument` is a handle into a function — so the argument here is attached
/// and unnamed. The output is the same for a reason that is in the routines,
/// not in the probe: `Value::print` sends an `Argument` to
/// `printAsOperand(OS, /*PrintType=*/true, MST)`, and neither overload calls
/// `ModuleSlotTracker::incorporateFunction` (only the `Instruction` and
/// `BasicBlock` arms of `Value::print` do), so `SlotTracker::getLocalSlot`
/// finds an empty `fMap` and answers -1 whether or not the argument has a
/// parent.
#[test]
fn an_argument_with_no_slot_prints_upstreams_badref() -> Result<(), IrError> {
    let m = module_new!("badref")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let arg = m.view(f).param(0)?;
    assert_eq!(format!("{arg}"), "i32 <badref>");

    // The `BasicBlock` arm of the same `if`, reached the same way: an
    // unnamed block printed as an *operand* with no tracker.
    let entry = m.view(f).append_basic_block(&m, "");
    let block = entry.to_erased();
    assert_eq!(format!("{block}"), "label <badref>");
    Ok(())
}
