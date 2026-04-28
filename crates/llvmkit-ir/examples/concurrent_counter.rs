//! Parser-1 victory lap. Builds two functions that exercise the
//! Session 1 instruction set:
//!
//! - `atomic_inc`: a release/acquire atomic increment of a 32-bit
//!   counter using `fence` + `atomicrmw add` + `fence`.
//! - `dispatch`: a 3-way `switch` on an opcode argument that selects
//!   `add`/`sub`/`mul` and returns the result. Uses the `Open`/`Closed`
//!   typestate on the [`SwitchInst`](llvmkit_ir::SwitchInst) handle.
//!
//! Target IR:
//!
//! ```llvm
//! ; ModuleID = 'concurrent_counter'
//! define i32 @atomic_inc(ptr %0) {
//! entry:
//!   fence release
//!   %old = atomicrmw add ptr %0, i32 1 monotonic
//!   fence acquire
//!   ret i32 %old
//! }
//!
//! define i32 @dispatch(i32 %0, i32 %1, i32 %2) {
//! entry:
//!   switch i32 %0, label %default [
//!     i32 0, label %do_add
//!     i32 1, label %do_sub
//!     i32 2, label %do_mul
//!   ]
//!
//! do_add:
//!   %r_add = add i32 %1, %2
//!   ret i32 %r_add
//!
//! do_sub:
//!   %r_sub = sub i32 %1, %2
//!   ret i32 %r_sub
//!
//! do_mul:
//!   %r_mul = mul i32 %1, %2
//!   ret i32 %r_mul
//!
//! default:
//!   ret i32 0
//! }
//! ```

use llvmkit_ir::{
    AtomicOrdering, AtomicRMWBinOp, AtomicRMWConfig, AtomicRMWFlags, IRBuilder, IntValue, IrError,
    Linkage, MaybeAlign, Module, PointerValue, SyncScope,
};

pub fn main() -> Result<(), IrError> {
    let m = Module::new("concurrent_counter");
    build_atomic_inc(&m)?;
    build_dispatch(&m)?;

    let text = format!("{m}");
    print!("{text}");
    Ok(())
}

/// `atomic_inc` --- a fence-bracketed `monotonic` atomic counter
/// increment. The pattern is the canonical fence-based decomposition
/// from <https://llvm.org/docs/Atomics.html>:
///
/// > A Monotonic load followed by an Acquire fence is roughly
/// > equivalent to an Acquire load, and a Monotonic store following a
/// > Release fence is roughly equivalent to a Release store.
///
/// Concretely:
/// 1. `fence release` --- publishes prior writes.
/// 2. `atomicrmw add ptr, i32 1 monotonic` --- atomic increment;
///    returns the old value.
/// 3. `fence acquire` --- ensures subsequent reads observe other
///    threads' releases.
pub fn build_atomic_inc(m: &Module<'_>) -> Result<(), IrError> {
    let i32_ty = m.i32_type();
    let ptr_ty = m.ptr_type(0);
    let fn_ty = m.fn_type(i32_ty, [ptr_ty.as_type()], false);
    let f = m.add_function::<i32>("atomic_inc", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i32>(m).position_at_end(entry);

    // fence release
    let _ = b.build_fence(AtomicOrdering::Release, SyncScope::System, "")?;

    // %old = atomicrmw add ptr %counter, i32 1 monotonic
    let counter: PointerValue = f.param(0)?.try_into()?;
    let one = i32_ty.const_int(1_i32);
    let old = b.build_atomicrmw(
        AtomicRMWBinOp::Add,
        counter,
        one,
        AtomicRMWConfig {
            ordering: AtomicOrdering::Monotonic,
            sync_scope: SyncScope::System,
            flags: AtomicRMWFlags::new(),
            align: MaybeAlign::NONE,
        },
        "old",
    )?;

    // fence acquire
    let _ = b.build_fence(AtomicOrdering::Acquire, SyncScope::System, "")?;

    // ret i32 %old
    let result: IntValue<i32> = old.as_instruction().as_value().try_into()?;
    b.build_ret(result)?;
    Ok(())
}

/// `dispatch` --- 3-way `switch` over an opcode value, selecting
/// `add` / `sub` / `mul` over two operands. Demonstrates the
/// [`Open`](llvmkit_ir::term_open_state::Open) /
/// [`Closed`](llvmkit_ir::term_open_state::Closed) typestate on
/// [`SwitchInst`](llvmkit_ir::SwitchInst): cases are added through the
/// chainable `add_case` API and the case list is sealed with
/// `finish()`.
pub fn build_dispatch(m: &Module<'_>) -> Result<(), IrError> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(
        i32_ty,
        [i32_ty.as_type(), i32_ty.as_type(), i32_ty.as_type()],
        false,
    );
    let f = m.add_function::<i32>("dispatch", fn_ty, Linkage::External)?;
    let entry = f.append_basic_block("entry");
    let do_add = f.append_basic_block("do_add");
    let do_sub = f.append_basic_block("do_sub");
    let do_mul = f.append_basic_block("do_mul");
    let default_bb = f.append_basic_block("default");

    // Each case body computes a single arithmetic op and returns.
    let a: IntValue<i32> = f.param(1)?.try_into()?;
    let b_op: IntValue<i32> = f.param(2)?.try_into()?;
    {
        let bb = IRBuilder::new_for::<i32>(m).position_at_end(do_add);
        let r = bb.build_int_add(a, b_op, "r_add")?;
        bb.build_ret(r)?;
    }
    {
        let bb = IRBuilder::new_for::<i32>(m).position_at_end(do_sub);
        let r = bb.build_int_sub(a, b_op, "r_sub")?;
        bb.build_ret(r)?;
    }
    {
        let bb = IRBuilder::new_for::<i32>(m).position_at_end(do_mul);
        let r = bb.build_int_mul(a, b_op, "r_mul")?;
        bb.build_ret(r)?;
    }
    {
        let bb = IRBuilder::new_for::<i32>(m).position_at_end(default_bb);
        bb.build_ret(0_i32)?;
    }

    // Build the entry switch. `add_case` returns the same `Open` handle
    // so the chain reads top-to-bottom; `finish` consumes it to a
    // `Closed` view that no longer accepts new cases at the type level.
    let op: IntValue<i32> = f.param(0)?.try_into()?;
    let entry_b = IRBuilder::new_for::<i32>(m).position_at_end(entry);
    let (_sealed, sw) = entry_b.build_switch(op, default_bb, "")?;
    let _closed = sw
        .add_case(i32_ty.const_int(0_i32), do_add)?
        .add_case(i32_ty.const_int(1_i32), do_sub)?
        .add_case(i32_ty.const_int(2_i32), do_mul)?
        .finish();
    Ok(())
}
