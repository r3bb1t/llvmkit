//! Auto-SSA twin of `examples/factorial.rs`: the SAME factorial loop,
//! built through [`SsaBuilder`]'s Braun on-the-fly SSA layer instead of
//! hand-rolled `build_int_phi` + `add_incoming` calls. Byte-parity locked
//! against the manual example by `tests/factorial_auto_ssa_example.rs`
//! (see that file for why this is the flagship D11 example-lock).
//!
//! Target IR -- IDENTICAL to `examples/factorial.rs`'s:
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
//! - `SsaBuilder::for_function` + `create_block`/`declare_int_var`.
//! - `def_int_var`/`use_int_var` reading and writing typed variables as
//!   if they were mutable locals -- no explicit phi construction.
//! - The phi RESULT names (`acc`, `i`) come from the declared variable
//!   names, matching the manual example's `build_int_phi::<i32, _>("acc")`
//!   naming exactly.
//! - Reading `acc`/`i` in `loop` BEFORE that block's own back-edge is
//!   recorded creates Braun's "incomplete" operandless phis; sealing
//!   `loop` AFTER its own `cond_br` back-edge completes both phis with
//!   the SAME incoming order the manual example hand-wired
//!   (`[ 1, %entry ], [ %next_acc, %loop ]`) -- entry-edge first because
//!   `entry`'s `cond_br(is_zero, base, loop)` records the loop-edge
//!   before `loop`'s own `cond_br(done, exit, loop)` records its
//!   self-edge.
//! - The `exit` block's `use_int_var(acc)` resolves through a SINGLE
//!   sealed predecessor (`loop`) straight to `%next_acc` -- no phi at
//!   all, Braun's no-redundant-phi property in action, matching the
//!   manual example's `ret i32 %next_acc`.
//!
//! Run:
//!
//! ```text
//! cargo run -p llvmkit-ir --example factorial_auto_ssa
//! ```

use llvmkit_ir::{IntPredicate, IrError, Linkage, Module, SsaBuilder};

pub fn build(m: &Module<'_>) -> Result<(), IrError> {
    let i32_ty = m.i32_type();
    let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
    let f = m
        .function_builder::<i32, _>("factorial", fn_ty)
        .linkage(Linkage::External)
        .param_name(0, "n")
        .build()?
        .with_typed_params::<(i32,)>()?;

    let mut b = SsaBuilder::for_function(m, f.as_function())?;

    // Same block names, same creation order as the manual example:
    // entry (auto-sealed), base, loop, exit.
    let entry = b.create_block("entry");
    let base = b.create_block("base");
    let loop_bb = b.create_block("loop");
    let exit = b.create_block("exit");

    // Variable names ARE the phi result names once the engine inserts
    // them -- declare with exactly the names the manual phis print as.
    let acc_var = b.declare_int_var::<i32, _>("acc");
    let i_var = b.declare_int_var::<i32, _>("i");

    let (n,) = f.params();

    // entry: %is_zero = icmp eq i32 %n, 0; def acc=1, i=n (this loop's
    // entry-edge incoming values belong to `entry`, the block they're
    // defined in); br i1 %is_zero, label %base, label %loop.
    let mut b = b.switch_to_block(entry)?;
    let is_zero = b
        .ins()
        .build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, n, 0_i32, "is_zero")?;
    b.def_int_var(acc_var, 1_i32)?;
    b.def_int_var(i_var, n)?;
    let mut b = b.cond_br(is_zero, base, loop_bb)?;
    // `base` has a single predecessor (entry) and is now fully known.
    b.seal_block(base)?;

    // base: ret i32 1
    let b = b.switch_to_block(base)?;
    let b = b.ret(1_i32)?;

    // loop: read i/acc BEFORE this block's own back-edge is recorded --
    // `loop` is still unsealed (its only known predecessor so far is
    // `entry`), so each read creates an operandless phi named after the
    // variable, head-inserted (each new phi anchors "before the current
    // first instruction", so reading `i` THEN `acc` prints `acc` first,
    // matching the manual example's phi order). `next_acc`/`next_i`/
    // `done` reuse the manual example's exact instruction names.
    let mut b = b.switch_to_block(loop_bb)?;
    let i = b.use_int_var(i_var)?;
    let acc = b.use_int_var(acc_var)?;
    let next_acc = b.ins().build_int_mul(acc, i, "next_acc")?;
    let next_i = b.ins().build_int_sub(i, 1_i32, "next_i")?;
    let done = b
        .ins()
        .build_int_cmp::<i32, _, _, _>(IntPredicate::Eq, next_i, 0_i32, "done")?;
    b.def_int_var(acc_var, next_acc)?;
    b.def_int_var(i_var, next_i)?;
    let mut b = b.cond_br(done, exit, loop_bb)?;
    // `loop`'s predecessor set (entry, loop-self) is now fully known:
    // sealing AFTER the back-edge completes both incomplete phis with
    // the entry-edge first, back-edge second -- matching the manual
    // example's `add_incoming(1, entry).add_incoming(next_acc, loop)`
    // chain order.
    b.seal_block(loop_bb)?;

    // exit: `acc`'s single sealed predecessor is `loop`, so this read
    // resolves directly to `%next_acc` with no phi -- ret i32 %next_acc.
    let mut b = b.switch_to_block(exit)?;
    b.seal_block(exit)?;
    let read = b.use_int_var(acc_var)?;
    let b = b.ret(read)?;

    b.finish()
}

pub fn main() {
    if let Err(e) = Module::with_new("factorial", |m| {
        build(&m)?;
        print!("{m}");
        Ok::<(), IrError>(())
    }) {
        eprintln!("error: {e:?}");
        std::process::exit(1);
    }
}
