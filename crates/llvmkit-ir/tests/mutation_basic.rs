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

use llvmkit_ir::{IRBuilder, Instruction, IntValue, IrError, Linkage, Module};

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
    let m = Module::new("u");
    let void_ty = m.void_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(void_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<()>("f", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
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
    let m = Module::new("e");
    let void_ty = m.void_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(void_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<()>("foo", fn_ty, Linkage::External)?;
    let bb = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(bb);
    let x: IntValue<i32> = f.param(0)?.try_into()?;

    let i1 = b.build_int_add(x, 0_i32, "i1")?;
    let i2 = b.build_int_add(x, 0_i32, "i2")?;
    let i3 = b.build_int_add(x, 0_i32, "i3")?;
    let ret = b.build_ret_void();

    // Pre-erase order: I1, I2, I3, Ret.
    let pre: Vec<_> = bb.instructions().map(|i| i.as_value()).collect();
    assert_eq!(pre.len(), 4);
    assert_eq!(pre[0], i1.as_value());
    assert_eq!(pre[1], i2.as_value());
    assert_eq!(pre[2], i3.as_value());
    assert_eq!(pre[3], ret.as_value());

    // Erase I2. Upstream: `I2->eraseFromParent(); I2 = nullptr;`
    let i2_inst = Instruction::try_from(i2.as_value())?;
    i2_inst.erase_from_parent();

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
}

/// Side-invariant from `EraseNoInvalidation`: erasing an instruction
/// also deregisters it from each operand's reverse use-list. Upstream
/// does not assert this directly because LLVM's use-list removal is a
/// side effect of `~Use::~Use()` running during deletion; we surface
/// it via `Value::num_uses` which is the llvmkit equivalent of
/// `Value::user_iterator` distance.
#[test]
fn erase_deregisters_from_operand_use_lists() -> Result<(), IrError> {
    let m = Module::new("e");
    let void_ty = m.void_type();
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(void_ty, [i32_ty.as_type()], false);
    let f = m.add_function::<()>("foo", fn_ty, Linkage::External)?;
    let bb = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(bb);
    let x: IntValue<i32> = f.param(0)?.try_into()?;

    let i1 = b.build_int_add(x, 0_i32, "i1")?;
    let i2 = b.build_int_add(x, 0_i32, "i2")?;
    let i3 = b.build_int_add(x, 0_i32, "i3")?;
    let _ret = b.build_ret_void();

    // Pre-erase: x has 3 users (one per add).
    assert_eq!(x.as_value().num_uses(), 3);

    let i2_inst = Instruction::try_from(i2.as_value())?;
    i2_inst.erase_from_parent();

    // Post-erase: x has 2 users (only the surviving adds).
    assert_eq!(x.as_value().num_uses(), 2);
    let users: Vec<_> = x.as_value().users().map(|i| i.as_value()).collect();
    assert!(users.contains(&i1.as_value()));
    assert!(users.contains(&i3.as_value()));
    assert!(!users.contains(&i2.as_value()));
    Ok(())
}
