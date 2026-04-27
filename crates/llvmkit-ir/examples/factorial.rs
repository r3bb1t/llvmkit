//! Phase E victory lap. Builds a factorial function as a single
//! loop with a phi-tracked accumulator and counter.
//!
//! Target IR:
//!
//! ```llvm
//! ; ModuleID = 'factorial'
//! define i32 @factorial(i32 %n) {
//! entry:
//!   %is_zero = icmp eq i32 %n, 0
//!   br i1 %is_zero, label %base, label %loop
//!
//! base:
//!   ret i32 1
//!
//! loop:
//!   %acc = phi i32 [ 1, %entry ], [ %next_acc, %loop ]
//!   %i = phi i32 [ %n, %entry ], [ %next_i, %loop ]
//!   %next_acc = mul i32 %acc, %i
//!   %next_i = sub i32 %i, 1
//!   %done = icmp eq i32 %next_i, 0
//!   br i1 %done, label %exit, label %loop
//!
//! exit:
//!   ret i32 %next_acc
//! }
//! ```
//!
//! Exercises:
//! - `i32` typed function builder.
//! - `IntoIntValue<i32>` lifting Rust scalars (`0_i32`, `1_i32`) at
//!   call sites without an intermediate constant binding.
//! - `build_int_cmp::<i32>` returning `IntValue<bool>`.
//! - `build_cond_br` consuming the `i1`.
//! - `build_int_phi` followed by chained `add_incoming` calls,
//!   mirroring `PHINode::addIncoming` (the loop-edge incoming value
//!   is defined later in the same block, so the chain interleaves
//!   with `build_int_mul` / `build_int_sub`).
//!
//! Run:
//!
//! ```text
//! cargo run -p llvmkit-ir --example factorial
//! ```

use llvmkit_ir::{IRBuilder, IntPredicate, IntValue, IrError, Linkage, Module};

pub fn build(m: &Module<'_>) -> Result<(), IrError> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m
        .function_builder::<i32>("factorial", fn_ty)
        .linkage(Linkage::External)
        .param_name(0, "n")
        .build()?;

    let entry = f.append_basic_block("entry");
    let base = f.append_basic_block("base");
    let loop_bb = f.append_basic_block("loop");
    let exit = f.append_basic_block("exit");

    let n: IntValue<i32> = f.param(0)?.try_into()?;

    // entry: %is_zero = icmp eq i32 %n, 0; br i1 %is_zero, ...
    let b = IRBuilder::new_for::<i32>(m).position_at_end(entry);
    let is_zero = b.build_int_cmp::<i32, _, _>(IntPredicate::Eq, n, 0_i32, "is_zero")?;
    b.build_cond_br(is_zero, base, loop_bb)?;

    // base: ret i32 1
    let b = IRBuilder::new_for::<i32>(m).position_at_end(base);
    b.build_ret(1_i32)?;

    // loop: create phis empty, build body, then patch phis with both edges.
    let b = IRBuilder::new_for::<i32>(m).position_at_end(loop_bb);
    let acc_phi = b.build_int_phi::<i32>("acc")?;
    let i_phi = b.build_int_phi::<i32>("i")?;
    let acc = acc_phi.as_int_value();
    let i = i_phi.as_int_value();
    let next_acc = b.build_int_mul(acc, i, "next_acc")?;
    let next_i = b.build_int_sub(i, 1_i32, "next_i")?;
    let done = b.build_int_cmp::<i32, _, _>(IntPredicate::Eq, next_i, 0_i32, "done")?;
    b.build_cond_br(done, exit, loop_bb)?;

    acc_phi
        .add_incoming(1_i32, entry)?
        .add_incoming(next_acc, loop_bb)?
        .finish();
    i_phi
        .add_incoming(n, entry)?
        .add_incoming(next_i, loop_bb)?
        .finish();

    // exit: ret i32 %next_acc
    let b = IRBuilder::new_for::<i32>(m).position_at_end(exit);
    b.build_ret(next_acc)?;
    Ok(())
}

pub fn main() {
    let m = Module::new("factorial");
    if let Err(e) = build(&m) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
    print!("{m}");
}
