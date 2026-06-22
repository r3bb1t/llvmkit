//! Mutation API coverage. Each test is ported from a specific
//! GoogleTest in `orig_cpp/.../llvm/unittests/IR/`. The IR shape is
//! reconstructed via the llvmkit IRBuilder because we do not yet ship a
//! `.ll` parser; assertions match what the upstream test asserts.
//!
//! `replace_all_uses_with` is deferred to a future session because it
//! requires an operand-mutation refactor (each `ValueId` slot becomes
//! a `Cell<ValueId>`); the change is mechanical but touches every
//! reader. Erase + use-list tracking is what DCE-lite (Session 4)
//! requires today.

use llvmkit_ir::metadata::{
    DebugMetadataOperand, DebugRecord, DebugVariableRecord, DebugVariableRecordKind, MetadataRef,
};
use llvmkit_ir::{IRBuilder, Instruction, IntValue, IrError, Linkage, Module, NoFolder};

/// Port of `unittests/IR/UseTest.cpp::TEST(UseTest, sort)` setup body.
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
/// Upstream then `sortUseList`s `%x` and asserts the iteration order.
/// We don't ship `sortUseList` (mutation primitive deferred); the
/// portable assertion is the registration count and that every user
/// in `X.users()` is the corresponding `add` instruction. That is
/// what `ASSERT_EQ(8u, I)` in the upstream loop validates.
#[test]
fn use_test_sort_setup_registers_eight_users() -> Result<(), IrError> {
    Module::with_new("u", |m| {
        let void_ty = m.void_type();
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(void_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);
        let x: IntValue<i32> = f.param(0)?.try_into()?;

        // Order matches the upstream string -- declaration order, not value index.
        let v0 = b.build_int_add(x, 0_i32, "v0")?;
        let v2 = b.build_int_add(x, 2_i32, "v2")?;
        let v5 = b.build_int_add(x, 5_i32, "v5")?;
        let v1 = b.build_int_add(x, 1_i32, "v1")?;
        let v3 = b.build_int_add(x, 3_i32, "v3")?;
        let v7 = b.build_int_add(x, 7_i32, "v7")?;
        let v6 = b.build_int_add(x, 6_i32, "v6")?;
        let v4 = b.build_int_add(x, 4_i32, "v4")?;
        b.build_ret_void();

        // Upstream: `ASSERT_EQ(8u, I)` after iterating `X.users()`.
        assert_eq!(x.as_value().num_uses(), 8);

        // The set of users is exactly the 8 adds (registration order).
        let users: Vec<_> = x.as_value().users().collect();
        assert_eq!(users.len(), 8);
        let expected_value_ids: Vec<_> = [v0, v2, v5, v1, v3, v7, v6, v4]
            .iter()
            .map(|iv| iv.as_value())
            .collect();
        let user_value_ids: Vec<_> = users.iter().map(|inst| inst.as_value()).collect();
        assert_eq!(user_value_ids, expected_value_ids);
        Ok(())
    })
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
    Module::with_new("e", |m| {
        let void_ty = m.void_type();
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(void_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<(), _>("foo", fn_ty, Linkage::External)?;
        let bb = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(bb);
        let x: IntValue<i32> = f.param(0)?.try_into()?;

        let i1 = b.build_int_add(x, 0_i32, "i1")?;
        let i2 = b.build_int_add(x, 0_i32, "i2")?;
        let i3 = b.build_int_add(x, 0_i32, "i3")?;
        let (_sealed, ret) = b.build_ret_void();

        // Pre-erase order: I1, I2, I3, Ret.
        let pre: Vec<_> = bb.instructions().map(|i| i.as_value()).collect();
        assert_eq!(pre.len(), 4);
        assert_eq!(pre[0], i1.as_value());
        assert_eq!(pre[1], i2.as_value());
        assert_eq!(pre[2], i3.as_value());
        assert_eq!(pre[3], ret.as_value());

        // Erase I2. Upstream: `I2->eraseFromParent(); I2 = nullptr;`
        let i2_inst = Instruction::try_from(i2.as_value())?;
        i2_inst.erase_from_parent(&m);

        // Post-erase: I1, I3, Ret. Upstream asserts via comesBefore +
        // iterator-equality; we assert the iteration order directly.
        let post: Vec<_> = bb.instructions().map(|i| i.as_value()).collect();
        assert_eq!(post.len(), 3);
        assert_eq!(post[0], i1.as_value());
        assert_eq!(post[1], i3.as_value());
        assert_eq!(post[2], ret.as_value());

        // Upstream's invariant `EXPECT_EQ(std::next(I1->getIterator()),
        // I3->getIterator())` -- I1's successor is now I3.
        Ok(())
    })
}
/// Mirrors `SymbolTableListTraitsImpl.h::removeNodeFromList`: erasing a named
/// instruction removes its value name from the owning function symbol table so
/// the name can be reused by a later instruction.
#[test]
fn erase_releases_local_name_for_reuse() -> Result<(), IrError> {
    Module::with_new("erase-name", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let arg: IntValue<i32> = f.param(0)?.try_into()?;

        let dead = b.build_int_add::<i32, _, _, _>(arg, 1_i32, "tmp")?;
        Instruction::try_from(dead.as_value())?.erase_from_parent(&m);
        let live = b.build_int_add::<i32, _, _, _>(arg, 2_i32, "tmp")?;
        b.build_ret(live)?;

        assert_eq!(live.name().as_deref(), Some("tmp"));
        let text = format!("{m}");
        assert!(!text.contains("%tmp1"), "{text}");
        assert!(text.contains("%tmp = add i32 %0, 2\n"), "{text}");
        Ok(())
    })
}

/// Mirrors `SymbolTableListTraitsImpl.h::transferNodesFromList`: inserting a
/// detached named instruction into a different function re-inserts the name into
/// the destination function symbol table and uniquifies conflicts there.
#[test]
fn detached_append_reinserts_and_uniques_against_destination() -> Result<(), IrError> {
    Module::with_new("move-name", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let from = m.add_function::<i32, _>("from", fn_ty, Linkage::External)?;
        let from_entry = from.append_basic_block(&m, "entry");
        let from_b = IRBuilder::with_folder(&m, NoFolder).position_at_end(from_entry);
        let moved_value = from_b.build_int_add::<i32, _, _, _>(
            i32_ty.const_int(1_i32),
            i32_ty.const_int(2_i32),
            "tmp",
        )?;
        let moved = Instruction::try_from(moved_value.as_value())?.detach_from_parent(&m);
        from_b.build_ret(i32_ty.const_zero())?;

        let to = m.add_function::<i32, _>("to", fn_ty, Linkage::External)?;
        let to_entry = to.append_basic_block(&m, "entry");
        let to_b = IRBuilder::with_folder(&m, NoFolder).position_at_end(to_entry);
        let existing = to_b.build_int_add::<i32, _, _, _>(
            i32_ty.const_int(3_i32),
            i32_ty.const_int(4_i32),
            "tmp",
        )?;
        let appended = moved.append_to(&m, &to_entry)?;
        let appended_value: IntValue<i32> = appended.try_into()?;
        to_b.build_ret(appended_value)?;

        assert_eq!(existing.name().as_deref(), Some("tmp"));
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
    })
}

/// Mirrors `Value.cpp::Value::setNameImpl`: a detached instruction has no
/// symbol table, so renaming it updates the carried `ValueData.name` without
/// re-registering that name in the old parent function.
#[test]
fn detached_set_name_updates_carried_name_without_old_parent_binding() -> Result<(), IrError> {
    Module::with_new("detached-rename", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, Vec::<llvmkit_ir::Type>::new(), false);
        let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);

        let original = b.build_int_add::<i32, _, _, _>(
            i32_ty.const_int(1_i32),
            i32_ty.const_int(2_i32),
            "tmp",
        )?;
        let detached = Instruction::try_from(original.as_value())?.detach_from_parent(&m);
        detached.as_value().set_name(&m, "renamed");

        let live = b.build_int_add::<i32, _, _, _>(
            i32_ty.const_int(3_i32),
            i32_ty.const_int(4_i32),
            "tmp",
        )?;
        let inserted = detached.append_to(&m, &entry)?;
        let inserted_value: IntValue<i32> = inserted.try_into()?;
        let sum = b.build_int_add::<i32, _, _, _>(live, inserted_value, "sum")?;
        b.build_ret(sum)?;

        assert_eq!(live.name().as_deref(), Some("tmp"));
        assert_eq!(inserted_value.name().as_deref(), Some("renamed"));
        let text = format!("{m}");
        assert!(text.contains("%tmp = add i32 3, 4\n"), "{text}");
        assert!(text.contains("%renamed = add i32 1, 2\n"), "{text}");
        assert!(!text.contains("%tmp1"), "{text}");
        Ok(())
    })
}

/// Side-invariant from `EraseNoInvalidation`: erasing an instruction
/// also deregisters it from each operand's reverse use-list. Upstream
/// does not assert this directly because LLVM's use-list removal is a
/// side effect of `~Use::~Use()` running during deletion; we surface
/// it via `Value::num_uses` which is the llvmkit equivalent of
/// `Value::user_iterator` distance.
#[test]
fn erase_deregisters_from_operand_use_lists() -> Result<(), IrError> {
    Module::with_new("e", |m| {
        let void_ty = m.void_type();
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(void_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<(), _>("foo", fn_ty, Linkage::External)?;
        let bb = f.append_basic_block(&m, "entry");
        let b = IRBuilder::new_for::<()>(&m).position_at_end(bb);
        let x: IntValue<i32> = f.param(0)?.try_into()?;

        let i1 = b.build_int_add(x, 0_i32, "i1")?;
        let i2 = b.build_int_add(x, 0_i32, "i2")?;
        let i3 = b.build_int_add(x, 0_i32, "i3")?;
        let _ret = b.build_ret_void();

        // Pre-erase: x has 3 users (one per add).
        assert_eq!(x.as_value().num_uses(), 3);

        let i2_inst = Instruction::try_from(i2.as_value())?;
        i2_inst.erase_from_parent(&m);

        // Post-erase: x has 2 users (only the surviving adds).
        assert_eq!(x.as_value().num_uses(), 2);
        let users: Vec<_> = x.as_value().users().map(|i| i.as_value()).collect();
        assert!(users.contains(&i1.as_value()));
        assert!(users.contains(&i3.as_value()));
        assert!(!users.contains(&i2.as_value()));
        Ok(())
    })
}

/// Mirrors `llvm/test/Assembler/metadata-use-uselistorder.ll` lines 5-13:
/// uses wrapped in metadata must still participate in value use tracking, but
/// are distinct from ordinary instruction users.
#[test]
fn metadata_constant_operand_counts_as_structural_value_use() -> Result<(), IrError> {
    Module::with_new("md-use", |m| {
        let i64_ty = m.i64_type();
        let c = i64_ty.const_int(4_i64);
        assert_eq!(c.as_value().num_uses(), 0);

        let md = m.metadata_constant(c);
        let tuple = m.metadata_tuple([MetadataRef(md)]);
        let idx = m.get_or_insert_named_metadata("uses");
        m.named_metadata_add_operand(idx, MetadataRef(tuple));

        assert_eq!(c.as_value().num_uses(), 1);
        assert_eq!(c.as_value().users().len(), 0);
        assert!(format!("{m}").contains("!0 = !{i64 4}"));
        Ok(())
    })
}

/// Mirrors `llvm/test/Assembler/metadata-use-uselistorder.ll` lines 10-13:
/// debug records live outside the instruction operand hierarchy, but value
/// operands inside them still contribute structural uses and must be removed
/// when the owning instruction is erased.
#[test]
fn debug_record_value_operand_counts_as_structural_use_and_erases() -> Result<(), IrError> {
    Module::with_new("dbg-use", |m| {
        let void_ty = m.void_type();
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(void_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let x: IntValue<i32> = f.param(0)?.try_into()?;
        assert_eq!(x.as_value().num_uses(), 0);
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let add = b.build_int_add::<i32, _, _, _>(
            i32_ty.const_int(1_i32),
            i32_ty.const_int(2_i32),
            "sum",
        )?;
        let inst = Instruction::try_from(add.as_value())?;
        let md = m.metadata_tuple(Vec::<MetadataRef>::new());
        inst.push_debug_record(DebugRecord::Variable(DebugVariableRecord::new(
            DebugVariableRecordKind::Value,
            DebugMetadataOperand::Value(x.as_value().id()),
            md,
            md,
            md,
        )));

        assert_eq!(x.as_value().num_uses(), 1);
        assert_eq!(x.as_value().users().len(), 0);
        let _ = b.build_ret_void();

        inst.erase_from_parent(&m);
        assert_eq!(x.as_value().num_uses(), 0);
        Ok(())
    })
}

/// Mirrors `llvm/test/Assembler/metadata-use-uselistorder.ll` lines 10-13:
/// debug-record `ValueAsMetadata` edges are outside the instruction operand
/// list, but `Value::replaceAllUsesWith` still rewrites them.
#[test]
fn debug_record_value_operand_is_rewritten_by_rauw() -> Result<(), IrError> {
    Module::with_new("dbg-rauw", |m| {
        let void_ty = m.void_type();
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(void_ty, [i32_ty.as_type()], false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
        let x: IntValue<i32> = f.param(0)?.try_into()?;

        let source = b.build_int_add::<i32, _, _, _>(x, i32_ty.const_int(1_i32), "source")?;
        let anchor = b.build_int_add::<i32, _, _, _>(
            i32_ty.const_int(2_i32),
            i32_ty.const_int(3_i32),
            "anchor",
        )?;
        let anchor_inst = Instruction::try_from(anchor.as_value())?;
        let md = m.metadata_tuple(Vec::<MetadataRef>::new());
        anchor_inst.push_debug_record(DebugRecord::Variable(DebugVariableRecord::new(
            DebugVariableRecordKind::Value,
            DebugMetadataOperand::Value(source.as_value().id()),
            md,
            md,
            md,
        )));

        let replacement = i32_ty.const_int(42_i32);
        assert_eq!(source.as_value().num_uses(), 1);
        assert_eq!(replacement.as_value().num_uses(), 0);

        let source_inst = Instruction::try_from(source.as_value())?;
        source_inst.replace_all_uses_with(&m, replacement)?;

        assert_eq!(source.as_value().num_uses(), 0);
        assert_eq!(replacement.as_value().num_uses(), 1);
        let records = anchor_inst.debug_records();
        let DebugRecord::Variable(record) = &records[0] else {
            panic!("expected variable debug record");
        };
        assert_eq!(
            record.location(),
            DebugMetadataOperand::Value(replacement.as_value().id())
        );
        let _ = b.build_ret_void();
        Ok(())
    })
}
