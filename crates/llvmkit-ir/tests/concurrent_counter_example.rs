//! Locks the `concurrent_counter` example's `.ll` output byte-for-byte.
//!
//! Closest upstream coverage:
//! - `atomic_inc`'s instruction sequence is the canonical fence-based
//!   decomposition documented in `llvm/docs/Atomics.html` (see the
//!   "Monotonic" / "Acquire" / "Release" sections); the printed shape
//!   is locked against the per-opcode fixtures in
//!   `test/Bitcode/compatibility.ll` (line 848 for `atomicrmw add ... monotonic`,
//!   lines 893-895 for `fence release` / `fence acquire`).
//! - `dispatch`'s `switch` print form mirrors `test/Bitcode/compatibility.ll`
//!   lines 1302-1310 (`switch <ty> %val, label %... [...]`).
//!
//! Doctrine D11: this is a `mirror` test --- it locks the AsmWriter
//! parity of the example binary against the same upstream fixtures
//! the per-opcode tests cite.

#[path = "../examples/concurrent_counter.rs"]
mod concurrent_counter_example;

use llvmkit_ir::{IrError, Module};

#[test]
fn concurrent_counter_example_emits_locked_ir() -> Result<(), IrError> {
    // Function-pointer coercion: marks `main` as used without running it.
    let _: fn() -> Result<(), IrError> = concurrent_counter_example::main;
    let m = Module::new("concurrent_counter");
    concurrent_counter_example::build_atomic_inc(&m)?;
    concurrent_counter_example::build_dispatch(&m)?;
    let actual = format!("{m}");
    let expected = "; ModuleID = 'concurrent_counter'\n\
define i32 @atomic_inc(ptr %0) {\n\
entry:\n  fence release\n  %old = atomicrmw add ptr %0, i32 1 monotonic\n  fence acquire\n  ret i32 %old\n}\n\n\
define i32 @dispatch(i32 %0, i32 %1, i32 %2) {\n\
entry:\n  switch i32 %0, label %default [\n    i32 0, label %do_add\n    i32 1, label %do_sub\n    i32 2, label %do_mul\n  ]\n\n\
do_add:\n  %r_add = add i32 %1, %2\n  ret i32 %r_add\n\n\
do_sub:\n  %r_sub = sub i32 %1, %2\n  ret i32 %r_sub\n\n\
do_mul:\n  %r_mul = mul i32 %1, %2\n  ret i32 %r_mul\n\n\
default:\n  ret i32 0\n}\n";
    assert_eq!(
        actual, expected,
        "concurrent_counter example output drifted"
    );
    Ok(())
}
