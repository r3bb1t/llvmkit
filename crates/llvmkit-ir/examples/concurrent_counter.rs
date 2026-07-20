//! Builds two functions that exercise the core instruction set:
//!
//! - `atomic_inc`: a release/acquire atomic increment of a 32-bit
//!   counter using `fence` + `atomicrmw add` + `fence`.
//! - `dispatch`: a 3-way `switch` on an opcode argument that selects
//!   `add`/`sub`/`mul` and returns the result. Uses the `Open`/`Closed`
//!   typestate on the `SwitchInst` handle.
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
    AtomicOrdering, AtomicRMWBinOp, AtomicRMWConfig, IRBuilder, IntValue, IrError, Linkage, Module,
    Ptr, SyncScope,
};

pub fn main() -> Result<(), IrError> {
    Module::with_new("concurrent_counter", |m| {
        build_atomic_inc(&m)?;
        build_dispatch(&m)?;

        let text = format!("{m}");
        print!("{text}");
        Ok(())
    })
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
pub fn build_atomic_inc<'ctx>(m: &Module<'ctx>) -> Result<(), IrError> {
    let i32_ty = m.i32_type();
    // `add_typed_function::<i32, (Ptr,)>` is the typed primary: the turbofish
    // is the whole schema (returns `i32`, takes one pointer), so there is no
    // separately built `FunctionType`.
    let f = m.add_typed_function::<i32, (Ptr,), _>("atomic_inc", Linkage::External)?;
    let entry = f.append_basic_block(m, "entry");
    let b = IRBuilder::at_end(entry);

    // fence release
    let _ = b.build_fence(AtomicOrdering::Release, SyncScope::System, "")?;

    // %old = atomicrmw add ptr %counter, i32 1 monotonic
    // `f.params()` hands back the parameter already typed as `PointerValue`
    // — no `f.param(0)?.try_into()?` narrowing step.
    let (counter,) = f.params();
    let one = i32_ty.const_int(1_i32);
    let old = b.build_atomicrmw(
        AtomicRMWBinOp::Add,
        counter,
        one,
        AtomicRMWConfig::new(AtomicOrdering::Monotonic, SyncScope::System),
        "old",
    )?;

    // fence acquire
    let _ = b.build_fence(AtomicOrdering::Acquire, SyncScope::System, "")?;

    // ret i32 %old
    let result: IntValue<i32> = old.into_erased().try_into()?;
    b.build_ret(result)?;
    Ok(())
}

/// `dispatch` --- 3-way `switch` over an opcode value, selecting
/// `add` / `sub` / `mul` over two operands. Demonstrates the
/// `Open` / `Closed` typestate on `SwitchInst`: cases are added
/// through the chainable `add_case` API and the case list is sealed
/// with `finish()`.
pub fn build_dispatch<'ctx>(m: &Module<'ctx>) -> Result<(), IrError> {
    let i32_ty = m.i32_type();
    // `add_typed_function::<i32, (i32, i32, i32)>` is the typed primary: the
    // turbofish *is* the signature, so no separate `FunctionType` is built.
    let f = m.add_typed_function::<i32, (i32, i32, i32), _>("dispatch", Linkage::External)?;
    let entry = f.append_basic_block(m, "entry");
    let do_add = f.append_basic_block(m, "do_add");
    let do_sub = f.append_basic_block(m, "do_sub");
    let do_mul = f.append_basic_block(m, "do_mul");
    let default_bb = f.append_basic_block(m, "default");
    let do_add_label = do_add.label();
    let do_sub_label = do_sub.label();
    let do_mul_label = do_mul.label();
    let default_label = default_bb.label();

    // `f.params()` returns the three arguments already typed as `IntValue<i32>`
    // in declaration order — no per-argument `f.param(n)?.try_into()?` narrowing.
    // Each case body computes a single arithmetic op and returns.
    let (op, a, b_op) = f.params();
    {
        let bb = IRBuilder::at_end(do_add);
        let r = bb.build_int_add(a, b_op, "r_add")?;
        bb.build_ret(r)?;
    }
    {
        let bb = IRBuilder::at_end(do_sub);
        let r = bb.build_int_sub(a, b_op, "r_sub")?;
        bb.build_ret(r)?;
    }
    {
        let bb = IRBuilder::at_end(do_mul);
        let r = bb.build_int_mul(a, b_op, "r_mul")?;
        bb.build_ret(r)?;
    }
    {
        let bb = IRBuilder::at_end(default_bb);
        bb.build_ret(0_i32)?;
    }

    // Build the entry switch. `add_case` returns the same `Open` handle
    // so the chain reads top-to-bottom; `finish` consumes it to a
    // `Closed` view that no longer accepts new cases at the type level.
    let entry_b = IRBuilder::at_end(entry);
    let (_sealed, sw) = entry_b.build_switch_dyn(op, default_label, "")?;
    let _closed = sw
        .add_case(i32_ty.const_int(0_i32), do_add_label)?
        .add_case(i32_ty.const_int(1_i32), do_sub_label)?
        .add_case(i32_ty.const_int(2_i32), do_mul_label)?
        .finish();
    Ok(())
}
