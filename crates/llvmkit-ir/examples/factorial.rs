//! Phase E victory lap. Builds a factorial function as a single loop
//! whose accumulator and counter are carried as the loop header block's
//! two parameters (head-phis), authored with the block-argument API.
//!
//! Target IR (the loop's accumulator and counter are the block's two *named*
//! parameters, so they print as the named head-phis `%acc`/`%i`):
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
//! - `int_cmp::<i32, _, _, _>` returning `IntValue<bool>`.
//! - `append_block_with_named_params` creating the `loop` header whose two
//!   named parameters ARE the accumulator/counter head-phis: `params[0]` is
//!   `%acc`, `params[1]` is `%i`.
//! - `cond_br_with_args` seeding those head-phis with block
//!   arguments: `entry` carries the initial values `[ 1, %n ]` down the
//!   loop edge, and the loop latch carries the back-edge values
//!   `[ %next_acc, %next_i ]`. The back-edge values are computed in the
//!   loop body BEFORE the latch terminator, so they dominate the branch
//!   that carries them.
//!
//! Run:
//!
//! ```text
//! cargo run -p llvmkit-ir --example factorial
//! ```

use llvmkit_ir::{
    IntPredicate, IntValue, IrBuilder, IrError, Linkage, Module, ModuleBrand, module_new,
};

pub fn build<B: ModuleBrand>(m: &Module<B>) -> Result<(), IrError> {
    let i32_ty = m.i32_type();
    let fn_ty = m.function_type(i32_ty, [i32_ty.as_type()]);
    let f = m
        .view(
            m.function_builder::<i32, _>("factorial", fn_ty)
                .linkage(Linkage::External)
                .param_name(0, "n")
                .build()?,
        )
        .with_typed_params::<(i32,)>()?;

    let entry = m.view(f).append_basic_block(m, "entry");
    let base = m.view(f).append_basic_block(m, "base");
    // The loop header's two parameters ARE the head-phis: `params[0]` carries
    // the accumulator (`%acc`), `params[1]` the counter (`%i`). Their incomings
    // arrive later as block arguments on the branches into `loop`.
    let bwp = IrBuilder::new_for::<i32>(m);
    let (loop_bb, params) = bwp.append_block_with_named_params(
        m.view(f).as_function(),
        [(i32_ty.as_type(), "acc"), (i32_ty.as_type(), "i")],
        "loop",
    )?;
    let exit = m.view(f).append_basic_block(m, "exit");
    let base_label = base.id();
    let loop_label = loop_bb.id();
    let exit_label = exit.id();

    let (n,) = m.view(f).params();

    // entry: %is_zero = icmp eq i32 %n, 0; then branch to `base` with no
    // arguments, or into `loop` carrying the header-phis' initial values
    // `[ acc = 1, i = %n ]`.
    let b = IrBuilder::at_end(entry);
    let is_zero = b.int_cmp::<i32, _, _, _>(IntPredicate::Eq, n, 0_i32, "is_zero")?;
    b.cond_br_with_args(
        is_zero,
        base_label,
        &[],
        loop_label,
        &[i32_ty.const_int(1_i32).as_erased(), n.as_erased()],
    )?;

    // base: ret i32 1
    let b = IrBuilder::at_end(base);
    b.ret(1_i32)?;

    // loop: `params[0]`/`params[1]` are the `%acc`/`%i` head-phis. Compute the
    // back-edge values from them, then re-enter `loop` carrying
    // `[ next_acc, next_i ]`. Those values are defined before the latch
    // terminator, so they dominate the branch that carries them.
    let b = IrBuilder::at_end(loop_bb);
    let acc: IntValue<'_, i32, _> = params[0].try_into()?;
    let i: IntValue<'_, i32, _> = params[1].try_into()?;
    let next_acc = b.int_mul(acc, i, "next_acc")?;
    let next_i = b.int_sub(i, 1_i32, "next_i")?;
    let done = b.int_cmp::<i32, _, _, _>(IntPredicate::Eq, next_i, 0_i32, "done")?;
    b.cond_br_with_args(
        done,
        exit_label,
        &[],
        loop_label,
        &[m.view(next_acc).as_erased(), m.view(next_i).as_erased()],
    )?;

    // exit: ret i32 %next_acc
    let b = IrBuilder::at_end(exit);
    b.ret(next_acc)?;
    Ok(())
}

/// The module is minted here rather than in `main` so `?` has a `Result` to
/// return to: `module_new!` is fallible (its brand is a registry key) and
/// `main` reports errors by hand.
fn emit() -> Result<(), IrError> {
    let m = module_new!("factorial")?;
    build(&m)?;
    print!("{m}");
    Ok(())
}

pub fn main() {
    if let Err(e) = emit() {
        eprintln!("error: {e:?}");
        std::process::exit(1);
    }
}
