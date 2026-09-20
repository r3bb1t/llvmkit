//! Mutation API coverage. Each test is ported from a specific
//! GoogleTest in `orig_cpp/.../llvm/unittests/IR/`. The IR shape is
//! reconstructed via the llvmkit IrBuilder because we do not yet ship a
//! `.ll` parser; assertions match what the upstream test asserts.
//!
//! `replace_all_uses_with` is deferred to a future session because it
//! requires an operand-mutation refactor (each `ValueSlot` slot becomes
//! a `Cell<ValueSlot>`); the change is mechanical but touches every
//! reader. Erase + use-list tracking is what DCE-lite (Session 4)
//! requires today.

use llvmkit_ir::metadata::{
    DebugMetadataOperand, DebugRecord, DebugVariableRecord, DebugVariableRecordKind, MetadataId,
};
use llvmkit_ir::{
    Dyn, IntValue, IrBuilder, IrError, Linkage, NoFolder, iter::BlockCursor, module_new,
};

/// Port of `unittests/IR/UseTest.cpp::TEST(UseTest, sort)`, whole.
/// Upstream:
/// ```text
/// define void @f(i32 %x) {
///   %v0 = add i32 %x, 0
///   %v2 = add i32 %x, 2
///   %v5 = add i32 %x, 5
///   %v1 = add i32 %x, 1
///   %v3 = add i32 %x, 3
///   %v7 = add i32 %x, 7
///   %v6 = add i32 %x, 6
///   %v4 = add i32 %x, 4
///   ret void
/// }
/// ```
/// It then `sortUseList`s `%x` by ascending user name and asserts
/// `X.users()` yields `v0 … v7`, re-sorts descending and asserts `v7 … v0`,
/// with `ASSERT_EQ(8u, I)` after each loop.
///
/// This used to be a *setup-only* port: its doc comment said "we don't ship
/// `sortUseList` (mutation primitive deferred)" and it asserted registration
/// order instead — an llvmkit-specific claim with no upstream counterpart,
/// and one that happened to be wrong, since upstream's use list is
/// newest-first. `Value::sort_use_list_by` closed the gap, so the port is
/// finished here rather than left approximated.
///
/// llvmkit builds the module with `IrBuilder` where upstream parses a string:
/// `llvmkit-ir` does not depend on the parser crate. The instruction sequence
/// is upstream's, in its order.
#[test]
fn use_test_sort() -> Result<(), IrError> {
    let m = module_new!("u")?;
    let void_ty = m.void_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(void_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;

    // Order matches the upstream string -- declaration order, not value index.
    for (addend, name) in [
        (0_i32, "v0"),
        (2, "v2"),
        (5, "v5"),
        (1, "v1"),
        (3, "v3"),
        (7, "v7"),
        (6, "v6"),
        (4, "v4"),
    ] {
        b.int_add(x, addend, name)?;
    }
    b.ret_void()?;

    let names = |value: llvmkit_ir::Value<'_, _>| -> Vec<String> {
        value
            .users()
            .map(|user| user.name().expect("every add is named"))
            .collect()
    };

    // `X.sortUseList([](L, R) { return L.getUser()->getName() < ...; })`
    x.as_erased()
        .sort_use_list_by(|left, right| left.name().cmp(&right.name()));
    let ascending = names(x.as_erased());
    assert_eq!(ascending.len(), 8, "upstream's ASSERT_EQ(8u, I)");
    assert_eq!(ascending, ["v0", "v1", "v2", "v3", "v4", "v5", "v6", "v7"]);

    // The same list re-sorted the other way; upstream asserts `v(7 - I)`.
    x.as_erased()
        .sort_use_list_by(|left, right| right.name().cmp(&left.name()));
    let descending = names(x.as_erased());
    assert_eq!(descending.len(), 8, "upstream's ASSERT_EQ(8u, I)");
    assert_eq!(descending, ["v7", "v6", "v5", "v4", "v3", "v2", "v1", "v0"]);
    Ok(())
}

/// The use-list order a *fresh* module has, before anything sorts it.
///
/// llvmkit-specific: upstream has no unit test pinning this directly — it is
/// asserted implicitly by `AsmWriter::predictValueUseListOrder`'s
/// `GetsReversed` logic and by every `verify-uselistorder` fixture. It is
/// pinned here because it is a contract three separate things depend on
/// (`uselistorder` index vectors, the printer's shuffle, and RAUW's merge
/// order), and because llvmkit got it backwards until wave 12: `Value::addUse`
/// forwards to `Use::addToList`, which makes each new use the **head**.
#[test]
fn use_list_reads_newest_first() -> Result<(), IrError> {
    let m = module_new!("u")?;
    let void_ty = m.void_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(void_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;

    b.int_add(x, 0_i32, "first")?;
    b.int_add(x, 1_i32, "second")?;
    b.int_add(x, 2_i32, "third")?;
    b.ret_void()?;

    let order: Vec<String> = x
        .as_erased()
        .users()
        .map(|user| user.name().expect("every add is named"))
        .collect();
    assert_eq!(order, ["third", "second", "first"]);
    Ok(())
}

/// Port of `unittests/IR/BasicBlockTest.cpp::TEST_F(InstrOrderInvalidationTest,
/// EraseNoInvalidation)`. Upstream constructs four `donothing` calls
/// (I1, I2, I3, Ret), erases I2, then asserts I1 still comes before
/// I3 in iteration order.
///
/// We substitute `add` instructions for `donothing` because we do not
/// ship intrinsics; the mutation invariant under test (erasing a
/// middle instruction leaves the surrounding ones in their original
/// relative order) is opcode-independent.
#[test]
fn erase_no_invalidation() -> Result<(), IrError> {
    let m = module_new!("e")?;
    let void_ty = m.void_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(void_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("foo", fn_ty, Linkage::External)?;
    let bb = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(bb);
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;

    let i1 = b.int_add(x, 0_i32, "i1")?;
    let i2 = b.int_add(x, 0_i32, "i2")?;
    let i3 = b.int_add(x, 0_i32, "i3")?;
    let bb = b.into_insert_block();

    // Pre-erase order before the terminator is emitted: I1, I2, I3.
    let pre: Vec<_> = bb.instructions().map(|i| i.to_erased()).collect();
    assert_eq!(pre.len(), 3);
    assert_eq!(pre[0], m.view(i1).as_erased());
    assert_eq!(pre[1], m.view(i2).as_erased());
    assert_eq!(pre[2], m.view(i3).as_erased());

    // Erase I2. Upstream: `I2->eraseFromParent(); I2 = nullptr;`
    let cursor = BlockCursor::at_start(bb);
    let (_, cursor) = cursor.step().expect("i1 instruction");
    let (i2_inst, cursor) = cursor.step().expect("i2 instruction");
    let bb = cursor.into_block();
    i2_inst.erase_from_parent(&m);
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(bb);
    let (bb, ret) = b.ret_void()?;

    // Post-erase: I1, I3, Ret. Upstream asserts via comesBefore +
    // iterator-equality; we assert the iteration order directly.
    let post: Vec<_> = bb.instructions().map(|i| i.to_erased()).collect();
    assert_eq!(post.len(), 3);
    assert_eq!(post[0], m.view(i1).as_erased());
    assert_eq!(post[1], m.view(i3).as_erased());
    assert_eq!(post[2], ret.to_erased());

    // Upstream's invariant `EXPECT_EQ(std::next(I1->getIterator()),
    // I3->getIterator())` -- I1's successor is now I3.
    Ok(())
}
/// Mirrors `SymbolTableListTraitsImpl.h::removeNodeFromList`: erasing a named
/// instruction removes its value name from the owning function symbol table so
/// the name can be reused by a later instruction.
#[test]
fn erase_releases_local_name_for_reuse() -> Result<(), IrError> {
    let m = module_new!("erase-name")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let arg: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;

    let _dead = b.int_add::<i32, _, _, _>(arg, 1_i32, "tmp")?;
    let block = b.into_insert_block();
    let (dead_inst, cursor) = BlockCursor::at_start(block)
        .step()
        .expect("dead instruction");
    dead_inst.erase_from_parent(&m);
    let block = cursor.into_block();
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(block);
    let live = b.int_add::<i32, _, _, _>(arg, 2_i32, "tmp")?;
    b.ret(live)?;

    assert_eq!(m.view(live).name().as_deref(), Some("tmp"));
    let text = format!("{m}");
    assert!(!text.contains("%tmp1"), "{text}");
    assert!(text.contains("%tmp = add i32 %0, 2\n"), "{text}");
    Ok(())
}

/// Mirrors `SymbolTableListTraitsImpl.h::transferNodesFromList`: inserting a
/// detached named instruction into a different function re-inserts the name into
/// the destination function symbol table and uniquifies conflicts there.
#[test]
fn detached_append_reinserts_and_uniques_against_destination() -> Result<(), IrError> {
    let m = module_new!("move-name")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, Vec::<llvmkit_ir::Type<'_, _>>::new());
    let from = m.add_function_dyn("from", fn_ty, Linkage::External)?;
    let from_entry = m.view(from).append_basic_block(&m, "entry");
    let from_b = IrBuilder::with_folder(&m, NoFolder).position_at_end(from_entry);
    let _moved_value =
        from_b.int_add::<i32, _, _, _>(i32_ty.const_int(1_i32), i32_ty.const_int(2_i32), "tmp")?;
    let from_block = from_b.into_insert_block();
    let (moved_inst, cursor) = BlockCursor::at_start(from_block)
        .step()
        .expect("moved instruction");
    let from_block = cursor.into_block();
    let moved = moved_inst.detach_from_parent(&m);
    let from_b = IrBuilder::with_folder(&m, NoFolder).position_at_end(from_block);
    from_b.ret(i32_ty.const_zero())?;

    let to = m.add_function_dyn("to", fn_ty, Linkage::External)?;
    let to_entry = m.view(to).append_basic_block(&m, "entry");
    let to_b = IrBuilder::with_folder(&m, NoFolder).position_at_end(to_entry);
    let existing =
        to_b.int_add::<i32, _, _, _>(i32_ty.const_int(3_i32), i32_ty.const_int(4_i32), "tmp")?;
    let appended = moved.append_to(&m, to_b.insert_block())?;
    let appended_value: IntValue<'_, i32, _> = appended.try_into()?;
    to_b.ret(appended_value)?;

    assert_eq!(m.view(existing).name().as_deref(), Some("tmp"));
    assert_eq!(appended_value.name().as_deref(), Some("tmp1"));
    let text = format!("{m}");
    assert!(
        text.contains("define i32 @from() {\nentry:\n  ret i32 0\n}\n"),
        "{text}"
    );
    assert!(
        text.contains("%tmp = add i32 3, 4\n  %tmp1 = add i32 1, 2\n  ret i32 %tmp1\n"),
        "{text}"
    );
    Ok(())
}

/// Mirrors `Value.cpp::Value::setNameImpl`: a detached instruction has no
/// symbol table, so renaming it updates the carried `ValueData.name` without
/// re-registering that name in the old parent function.
#[test]
fn detached_set_name_updates_carried_name_without_old_parent_binding() -> Result<(), IrError> {
    let m = module_new!("detached-rename")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, Vec::<llvmkit_ir::Type<'_, _>>::new());
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);

    let _original =
        b.int_add::<i32, _, _, _>(i32_ty.const_int(1_i32), i32_ty.const_int(2_i32), "tmp")?;
    let block = b.into_insert_block();
    let (detached_inst, cursor) = BlockCursor::at_start(block)
        .step()
        .expect("original instruction");
    let detached = detached_inst.detach_from_parent(&m);
    let block = cursor.into_block();
    detached.to_erased().set_name(&m, "renamed");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(block);
    let live =
        b.int_add::<i32, _, _, _>(i32_ty.const_int(3_i32), i32_ty.const_int(4_i32), "tmp")?;
    let inserted = detached.append_to(&m, b.insert_block())?;
    let inserted_value: IntValue<'_, i32, _> = inserted.try_into()?;
    let sum = b.int_add::<i32, _, _, _>(live, inserted_value, "sum")?;
    b.ret(sum)?;

    assert_eq!(m.view(live).name().as_deref(), Some("tmp"));
    assert_eq!(inserted_value.name().as_deref(), Some("renamed"));
    let text = format!("{m}");
    assert!(text.contains("%tmp = add i32 3, 4\n"), "{text}");
    assert!(text.contains("%renamed = add i32 1, 2\n"), "{text}");
    assert!(!text.contains("%tmp1"), "{text}");
    Ok(())
}

/// Side-invariant from `EraseNoInvalidation`: erasing an instruction
/// also deregisters it from each operand's reverse use-list. Upstream
/// does not assert this directly because LLVM's use-list removal is a
/// side effect of `~Use::~Use()` running during deletion; we surface
/// it via `Value::num_uses` which is the llvmkit equivalent of
/// `Value::user_iterator` distance.
#[test]
fn erase_deregisters_from_operand_use_lists() -> Result<(), IrError> {
    let m = module_new!("e")?;
    let void_ty = m.void_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(void_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("foo", fn_ty, Linkage::External)?;
    let bb = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(bb);
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;

    let i1 = b.int_add(x, 0_i32, "i1")?;
    let i2 = b.int_add(x, 0_i32, "i2")?;
    let i3 = b.int_add(x, 0_i32, "i3")?;
    let bb = b.into_insert_block();

    // Pre-erase: x has 3 users (one per add).
    assert_eq!(x.as_erased().num_uses(), 3);

    let cursor = BlockCursor::at_start(bb);
    let (_, cursor) = cursor.step().expect("i1 instruction");
    let (i2_inst, cursor) = cursor.step().expect("i2 instruction");
    let bb = cursor.into_block();
    i2_inst.erase_from_parent(&m);
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(bb);
    let _ = b.ret_void();

    // Post-erase: x has 2 users (only the surviving adds).
    assert_eq!(x.as_erased().num_uses(), 2);
    let users: Vec<_> = x.as_erased().users().map(|i| i.to_erased()).collect();
    assert!(users.contains(&m.view(i1).as_erased()));
    assert!(users.contains(&m.view(i3).as_erased()));
    assert!(!users.contains(&m.view(i2).as_erased()));
    Ok(())
}

/// Mirrors `llvm/test/Assembler/metadata-use-uselistorder.ll` lines 5-13:
/// uses wrapped in metadata must still participate in value use tracking, but
/// are distinct from ordinary instruction users.
#[test]
fn metadata_constant_operand_counts_as_structural_value_use() -> Result<(), IrError> {
    let m = module_new!("md-use")?;
    let i64_ty = m.i64_type();
    let c = i64_ty.const_int(4_i64);
    assert_eq!(c.as_erased().num_uses(), 0);

    let md = m.metadata_constant(c)?;
    let tuple = m.metadata_tuple([md])?;
    let idx = m.get_or_insert_named_metadata("uses");
    m.named_metadata_add_operand(idx, tuple)?;

    assert_eq!(c.as_erased().num_uses(), 1);
    assert_eq!(c.as_erased().users().len(), 0);
    assert!(format!("{m}").contains("!0 = !{i64 4}"));
    Ok(())
}

/// Mirrors `lib/IR/Instruction.cpp::Instruction::moveBefore` and
/// `Instruction::moveAfter`: moving an instruction relative to itself is a
/// no-op and must not detach it from its parent block.
#[test]
fn self_anchored_instruction_moves_are_no_ops() -> Result<(), IrError> {
    let m = module_new!("self-move")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, Vec::<llvmkit_ir::Type<'_, _>>::new());
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let builder = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let a =
        builder.int_add::<i32, _, _, _>(i32_ty.const_int(1_i32), i32_ty.const_int(2_i32), "a")?;
    let b = builder.int_add::<i32, _, _, _>(a, i32_ty.const_int(3_i32), "b")?;

    let block = builder.into_insert_block();
    let cursor = BlockCursor::at_start(block);
    let (a_inst, cursor) = cursor.step().expect("a instruction");
    let a_anchor = a_inst.placed();
    a_inst.move_before(&m, a_anchor)?;
    let (b_inst, cursor) = cursor.step().expect("b instruction");
    let b_anchor = b_inst.placed();
    b_inst.move_after(&m, b_anchor)?;

    let block = cursor.into_block();
    let builder = IrBuilder::with_folder(&m, NoFolder).position_at_end(block);
    builder.ret(b)?;
    let text = format!("{m}");
    assert!(text.contains("%a = add i32 1, 2"), "{text}");
    assert!(text.contains("%b = add i32 %a, 3"), "{text}");
    assert!(text.contains("ret i32 %b"), "{text}");
    Ok(())
}

/// Mirrors `llvm/test/Assembler/metadata-use-uselistorder.ll` lines 10-13:
/// debug records live outside the instruction operand hierarchy, but value
/// operands inside them still contribute structural uses and must be removed
/// when the owning instruction is erased.
#[test]
fn debug_record_value_operand_counts_as_structural_use_and_erases() -> Result<(), IrError> {
    let m = module_new!("dbg-use")?;
    let void_ty = m.void_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(void_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    assert_eq!(x.as_erased().num_uses(), 0);
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let _add =
        b.int_add::<i32, _, _, _>(i32_ty.const_int(1_i32), i32_ty.const_int(2_i32), "sum")?;
    let block = b.into_insert_block();
    let (inst, cursor) = BlockCursor::at_start(block)
        .step()
        .expect("sum instruction");
    let md = m.metadata_tuple(Vec::<MetadataId<_>>::new())?;
    inst.push_debug_record(
        &m,
        DebugRecord::Variable(DebugVariableRecord::new(
            DebugVariableRecordKind::Value,
            DebugMetadataOperand::Value(x.as_erased().id()),
            md,
            md,
            md,
        )),
    )?;

    assert_eq!(x.as_erased().num_uses(), 1);
    assert_eq!(x.as_erased().users().len(), 0);
    let block = cursor.into_block();
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(block);
    let _ = b.ret_void();

    inst.erase_from_parent(&m);
    assert_eq!(x.as_erased().num_uses(), 0);
    Ok(())
}

/// Mirrors `llvm/test/Assembler/metadata-use-uselistorder.ll` lines 10-13:
/// debug-record `ValueAsMetadata` edges are outside the instruction operand
/// list, but `Value::replaceAllUsesWith` still rewrites them.
#[test]
fn debug_record_value_operand_is_rewritten_by_rauw() -> Result<(), IrError> {
    let m = module_new!("dbg-rauw")?;
    let void_ty = m.void_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(void_ty, [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;

    let source = b.int_add::<i32, _, _, _>(x, i32_ty.const_int(1_i32), "source")?;
    let _anchor =
        b.int_add::<i32, _, _, _>(i32_ty.const_int(2_i32), i32_ty.const_int(3_i32), "anchor")?;
    let block = b.into_insert_block();
    let cursor = BlockCursor::at_start(block);
    let (source_inst, cursor) = cursor.step().expect("source instruction");
    let (anchor_inst, cursor) = cursor.step().expect("anchor instruction");
    let md = m.metadata_tuple(Vec::<MetadataId<_>>::new())?;
    anchor_inst.push_debug_record(
        &m,
        DebugRecord::Variable(DebugVariableRecord::new(
            DebugVariableRecordKind::Value,
            DebugMetadataOperand::Value(m.view(source).as_erased().id()),
            md,
            md,
            md,
        )),
    )?;

    let replacement = i32_ty.const_int(42_i32);
    assert_eq!(m.view(source).as_erased().num_uses(), 1);
    assert_eq!(replacement.as_erased().num_uses(), 0);

    source_inst.replace_all_uses_with(&m, replacement)?;

    assert_eq!(m.view(source).as_erased().num_uses(), 0);
    assert_eq!(replacement.as_erased().num_uses(), 1);
    let records: Vec<_> = anchor_inst.debug_records().collect();
    let DebugRecord::Variable(record) = &records[0] else {
        panic!("expected variable debug record");
    };
    assert_eq!(
        record.location(),
        DebugMetadataOperand::Value(replacement.as_erased().id())
    );
    let block = cursor.into_block();
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(block);
    let _ = b.ret_void();
    Ok(())
}

/// `BasicBlock::split_at` refuses a split point that lives in another block of
/// the same function, and refuses it before anything is appended: the module
/// prints exactly as it did before the call.
///
/// No upstream counterpart: `BasicBlock::splitBasicBlock`
/// (`lib/IR/BasicBlock.cpp`) takes the split point as an iterator into the
/// block's own instruction list, so a split point from another block has no
/// error path there — it asserts only that the block is terminated and that the
/// iterator is not `end()`. llvmkit takes an `InstructionView`, which can name
/// any instruction, so it returns `IrError::InvalidOperation`, and a refused
/// call must not mutate. `split_at` used to append the new block
/// before looking for the split point, so this call returned the error and
/// left an empty `entry.split` block in the function.
#[test]
fn split_at_refuses_an_instruction_of_another_block_without_mutating() -> Result<(), IrError> {
    let m = module_new!("split-at-foreign-block")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type_no_parameters(i32_ty);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let next = m.view(f).append_basic_block(&m, "next");

    // entry: br next    next: ret 0
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    b.br(next.id())?;
    let b2 = IrBuilder::new_for::<Dyn>(&m).position_at_end(next);
    b2.ret(i32_ty.const_int(0_u32))?;
    let printed_before = format!("{m}");

    let mut blocks = m.view(f).basic_blocks();
    let entry = blocks.next().expect("entry was appended first");
    let next = blocks.next().expect("next was appended second");
    let foreign = next.terminator().expect("next is terminated by the ret");

    let result = entry.split_at(&m, &foreign, "entry.split");
    assert!(
        matches!(
            result,
            Err(IrError::InvalidOperation {
                message: "split instruction is not in this block"
            })
        ),
        "split_at must refuse a split point outside the block"
    );
    assert_eq!(
        format!("{m}"),
        printed_before,
        "a refused split must not mutate the module"
    );
    Ok(())
}

/// `BasicBlock::splitBasicBlock` (`lib/IR/BasicBlock.cpp`) opens with
/// `assert(getTerminator() && "Can't use splitBasicBlock on degenerate BB!")`.
/// `split_at` is only callable on a `Terminated` handle, but that typestate is
/// not proof on its own: `FunctionValue::basic_blocks` hands out `Terminated`
/// views of every block. So the assert is also a runtime refusal, raised before
/// anything is appended or moved.
///
/// llvmkit-specific: upstream crashes here rather than returning, so there is
/// no upstream test to port; refusing is hardening, not a divergence.
#[test]
fn split_at_refuses_a_block_without_a_terminator_without_mutating() -> Result<(), IrError> {
    let m = module_new!("split-at-degenerate-block")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type_no_parameters(i32_ty);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let _x = b.int_add::<i32, _, _, _>(i32_ty.const_int(1_u32), i32_ty.const_int(2_u32), "x")?;
    let printed_before = format!("{m}");

    let entry = m.view(f).basic_blocks().next().expect("entry was appended");
    let x = entry.instructions().next().expect("entry holds %x");

    let result = entry.split_at(&m, &x, "entry.split");
    assert!(
        matches!(
            result,
            Err(IrError::InvalidOperation {
                message: "Can't use splitBasicBlock on degenerate BB!"
            })
        ),
        "split_at must refuse a block without a terminator"
    );
    assert_eq!(
        format!("{m}"),
        printed_before,
        "a refused split must not mutate the module"
    );
    Ok(())
}

/// `BasicBlock::splitBasicBlockBefore` (`lib/IR/BasicBlock.cpp`) opens with
/// `assert(getTerminator() && "Can't use splitBasicBlockBefore on degenerate
/// BB!")`. As for `split_at`, the `Terminated` handle `basic_blocks` hands out
/// is no proof, so `split_before` refuses at run time before anything is
/// created, moved or retargeted.
///
/// llvmkit-specific: upstream crashes here rather than returning, so there is
/// no upstream test to port; refusing is hardening, not a divergence.
#[test]
fn split_before_refuses_a_block_without_a_terminator_without_mutating() -> Result<(), IrError> {
    let m = module_new!("split-before-degenerate-block")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type_no_parameters(i32_ty);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let _x = b.int_add::<i32, _, _, _>(i32_ty.const_int(1_u32), i32_ty.const_int(2_u32), "x")?;
    let printed_before = format!("{m}");

    let entry = m.view(f).basic_blocks().next().expect("entry was appended");
    let x = entry.instructions().next().expect("entry holds %x");

    let result = entry.split_before(&m, &x, "entry.head");
    assert!(
        matches!(
            result,
            Err(IrError::InvalidOperation {
                message: "Can't use splitBasicBlockBefore on degenerate BB!"
            })
        ),
        "split_before must refuse a block without a terminator"
    );
    assert_eq!(
        format!("{m}"),
        printed_before,
        "a refused split must not mutate the module"
    );
    Ok(())
}

/// `Instruction::removeFromParent` (`lib/IR/Instruction.cpp`) takes the
/// instruction out of its block's list, and `Instruction::getParent` answers
/// null for it afterwards: the `Parent` pointer is cleared by the
/// `ilist` traits' `removeNodeFromList`. llvmkit's `detach_from_parent` must
/// leave the same state — a detached instruction names no block — and
/// re-inserting it must name the new one.
///
/// llvmkit-specific: no upstream unit test asserts the null parent
/// (`rg -n "removeFromParent" unittests/IR/*.cpp` reaches only
/// `BasicBlockTest.cpp`'s `RemoveNoInvalidation`, which asserts instruction
/// order). The attached reads on either side are the positive controls.
#[test]
fn detach_from_parent_leaves_the_instruction_in_no_block() -> Result<(), IrError> {
    let m = module_new!("detached-parent")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type_no_parameters(i32_ty);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let _x = b.int_add::<i32, _, _, _>(i32_ty.const_int(1_u32), i32_ty.const_int(2_u32), "x")?;
    let block = b.into_insert_block();
    let block_id = block.id();
    let (instruction, cursor) = BlockCursor::at_start(block).step().expect("entry holds %x");
    let block = cursor.into_block();

    // Positive control: attached, it names the block it is in.
    assert_eq!(instruction.as_view().parent(), Some(block_id.as_dyn()));

    let detached = instruction.detach_from_parent(&m);
    assert_eq!(
        detached.as_view().parent(),
        None,
        "a detached instruction is in no block"
    );

    let reattached = detached.append_to(&m, &block)?;
    assert_eq!(
        reattached.as_view().parent(),
        Some(block_id.as_dyn()),
        "re-inserting names the new block"
    );
    Ok(())
}

/// `IRBuilderBase::SetInsertPoint(Instruction *I)` (`IR/IRBuilder.h`) reads
/// `I->getParent()` and inserts into it. For an instruction in no block that
/// pointer is null and upstream dereferences it; llvmkit refuses with
/// [`IrError::InstructionHasNoParent`] before anything is created (D10).
///
/// llvmkit-specific: upstream has no test for the null case, because there is
/// no defined behaviour to test. The attached anchor is the positive control.
#[test]
fn position_before_refuses_an_anchor_in_no_block_without_mutating() -> Result<(), IrError> {
    let m = module_new!("position-before-detached")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty.as_type(), [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let _x = b.int_add(a, 1_i32, "x")?;
    let block = b.into_insert_block();
    let (anchor, cursor) = BlockCursor::at_start(block).step().expect("entry holds %x");
    let block = cursor.into_block();

    // Positive control: an anchor in a block positions the builder, and the
    // instruction it emits lands before that anchor. Its left operand is a
    // parameter, so this builder's default folder cannot turn the add into a
    // constant and leave no instruction behind to find.
    let positioned = IrBuilder::new_for::<Dyn>(&m).position_before(anchor.placed())?;
    let _before = positioned.int_add(a, 2_i32, "before")?;
    assert_eq!(
        block
            .instructions()
            .map(|instruction| instruction.name())
            .collect::<Vec<_>>(),
        vec![Some("before".to_string()), Some("x".to_string())],
        "the positive control's instruction lands before the anchor"
    );

    // The witness is minted while the anchor is still in its block, then the
    // anchor leaves it: the one case a `PlacedInstruction` cannot rule out,
    // because nothing freezes the layout between the mint and the call.
    let stale = anchor.placed();
    let detached = anchor.detach_from_parent(&m);
    assert_eq!(
        detached.as_view().placed().map(|placed| placed.block()),
        None,
        "a detached instruction mints no witness"
    );
    let printed_before = format!("{m}");
    let refused = IrBuilder::new_for::<Dyn>(&m).position_before(stale);
    assert!(
        matches!(refused, Err(IrError::InstructionHasNoParent)),
        "a stale witness must be refused"
    );
    assert_eq!(
        format!("{m}"),
        printed_before,
        "a refused positioning must not mutate the module"
    );
    Ok(())
}

/// `Instruction::moveBefore` and `Instruction::insertBefore`
/// (`lib/IR/Instruction.cpp`) both read `InsertPos->getParent()` — null for an
/// anchor that is in no block. llvmkit refuses with
/// [`IrError::InstructionHasNoParent`], and the refusal happens before the
/// moved instruction leaves its own block, so a rejected move strands nothing.
///
/// llvmkit-specific, as above: the anchored moves in
/// `self_anchored_instruction_moves_are_no_ops` are the positive control for
/// the same entry points.
#[test]
fn a_move_or_insert_refuses_an_anchor_in_no_block_without_mutating() -> Result<(), IrError> {
    let m = module_new!("move-detached-anchor")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type_no_parameters(i32_ty);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let _x = b.int_add::<i32, _, _, _>(i32_ty.const_int(1_u32), i32_ty.const_int(2_u32), "x")?;
    let _y = b.int_add::<i32, _, _, _>(i32_ty.const_int(3_u32), i32_ty.const_int(4_u32), "y")?;
    let block = b.into_insert_block();
    let (anchor, cursor) = BlockCursor::at_start(block).step().expect("entry holds %x");
    let (mover, cursor) = cursor.step().expect("entry holds %y");
    let block = cursor.into_block();
    // As in the positioning test: the witness is minted while the anchor is
    // in its block, and the anchor then leaves it.
    let stale = anchor.placed();
    let detached_anchor = anchor.detach_from_parent(&m);
    let printed_before = format!("{m}");

    let refused = mover.move_before(&m, stale);
    assert!(
        matches!(refused, Err(IrError::InstructionHasNoParent)),
        "moving before a stale witness must be refused"
    );
    assert_eq!(
        format!("{m}"),
        printed_before,
        "a refused move must leave the mover in its block"
    );

    let (mover, cursor) = BlockCursor::at_start(block).step().expect("entry holds %y");
    let _ = cursor.into_block();
    assert!(
        detached_anchor.as_view().placed().is_none(),
        "a detached anchor mints no witness"
    );
    let refused = mover.move_after(&m, stale);
    assert!(
        matches!(refused, Err(IrError::InstructionHasNoParent)),
        "moving after a stale witness must be refused"
    );
    assert_eq!(format!("{m}"), printed_before, "still unmoved");
    Ok(())
}

/// `BasicBlock::placed_instructions` mints a witness per instruction without a
/// check, because the block is the one being walked rather than a field read.
///
/// **No upstream counterpart.** `BasicBlock::iterator`
/// (`include/llvm/IR/BasicBlock.h`) yields `Instruction &`, whose
/// `getParent()` upstream's callers dereference without asking, so there is no
/// upstream behaviour to port. The claim is llvmkit's own, in three parts:
/// every witness the walk yields names the walked block; the walk agrees with
/// [`BasicBlock::instructions`] element for element; and a witness taken
/// straight from the walk drives `position_before` with no `Option` in
/// between, which is what distinguishes this mint from
/// `InstructionView::placed`.
#[test]
fn placed_instructions_mint_a_witness_for_the_block_being_walked() -> Result<(), IrError> {
    let m = module_new!("placed-instructions")?;
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty.as_type(), [i32_ty.as_type()]);
    let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
    let a: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let _x = b.int_add(a, 1_i32, "x")?;
    let _y = b.int_add(a, 2_i32, "y")?;
    let block = b.into_insert_block();
    let block_id = block.id();

    assert_eq!(
        block.placed_instructions().len(),
        2,
        "the walk covers the whole block"
    );
    for placed in block.placed_instructions() {
        assert_eq!(
            placed.block(),
            block_id.as_dyn(),
            "a witness from the walk names the walked block"
        );
    }
    assert_eq!(
        block
            .placed_instructions()
            .map(|placed| placed.instruction())
            .collect::<Vec<_>>(),
        block.instructions().collect::<Vec<_>>(),
        "the witness walk and the view walk agree element for element"
    );

    // The mint needs no `ok_or`: `position_before` takes what the walk yields.
    let anchor = block
        .placed_instructions()
        .next()
        .expect("entry holds two instructions");
    let positioned = IrBuilder::new_for::<Dyn>(&m).position_before(anchor)?;
    let _first = positioned.int_add(a, 3_i32, "first")?;
    assert_eq!(
        block
            .instructions()
            .map(|instruction| instruction.name())
            .collect::<Vec<_>>(),
        vec![
            Some("first".to_string()),
            Some("x".to_string()),
            Some("y".to_string())
        ],
        "the emitted instruction lands before the anchor the walk yielded"
    );
    Ok(())
}
