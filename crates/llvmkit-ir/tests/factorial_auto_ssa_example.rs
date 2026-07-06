//! Locks the auto-SSA factorial example's `.ll` output byte-for-byte
//! against the SAME expected string `tests/factorial_example.rs` locks
//! for the manual (hand-rolled phi) factorial example.
//!
//! ## Upstream provenance
//!
//! example: locks output of `examples/factorial_auto_ssa.rs`. Closest
//! upstream functional pattern: `unittests/IR/IRBuilderTest.cpp`
//! (recursive / looped function building via IRBuilder) -- same
//! reference `tests/factorial_example.rs` cites, since this is the same
//! IR built through a different construction path.
//!
//! This is the flagship D11 example-lock: [`SsaBuilder`]'s Braun
//! on-the-fly SSA engine and the manual `build_int_phi`/`add_incoming`
//! construction in `examples/factorial.rs` print IDENTICAL `.ll` for the
//! same factorial loop. The expected string below is DUPLICATED from
//! `tests/factorial_example.rs::factorial_example_emits_locked_ir`
//! rather than shared via a common `include!` -- the manual test's own
//! expected string is an inline literal (not itself extracted to a
//! shared file), so duplicating with this cross-reference comment is the
//! lower-friction choice that matches the existing structure; a future
//! edit to either target string is expected to intentionally touch both
//! call sites (a divergence would mean the parity claim itself broke).
//!
//! Mirrors the `cpu_state_add_example.rs` / `factorial_example.rs`
//! pattern: `#[path]`-import the example body, build into a fresh
//! module, assert on the format result.

#[path = "../examples/factorial_auto_ssa.rs"]
mod factorial_auto_ssa_example;

use llvmkit_ir::{IrError, Module};

/// example: locks `examples/factorial_auto_ssa.rs` output byte-for-byte
/// against `tests/factorial_example.rs`'s target string. Closest upstream
/// functional reference: `unittests/IR/IRBuilderTest.cpp` (loop+phi
/// construction patterns) -- see that test's own doc comment.
#[test]
fn factorial_auto_ssa_example_emits_locked_ir() -> Result<(), IrError> {
    Module::with_new("factorial", |m| {
        // Function-pointer coercion: marks `main` as used without running it.
        let _: fn() = factorial_auto_ssa_example::main;
        factorial_auto_ssa_example::build(&m)?;
        let actual = format!("{m}");
        // Cross-reference: this literal is the SAME expected string as
        // `tests/factorial_example.rs::factorial_example_emits_locked_ir`.
        // Any change to either target string must update both.
        let expected = "; ModuleID = 'factorial'\n\
    define i32 @factorial(i32 %n) {\n\
    entry:\n  %is_zero = icmp eq i32 %n, 0\n  br i1 %is_zero, label %base, label %loop\n\n\
    base:\n  ret i32 1\n\n\
    loop:\n  %acc = phi i32 [ 1, %entry ], [ %next_acc, %loop ]\n  %i = phi i32 [ %n, %entry ], [ %next_i, %loop ]\n  %next_acc = mul i32 %acc, %i\n  %next_i = sub i32 %i, 1\n  %done = icmp eq i32 %next_i, 0\n  br i1 %done, label %exit, label %loop\n\n\
    exit:\n  ret i32 %next_acc\n\
    }\n";
        assert_eq!(actual, expected, "got:\n{actual}");
        Ok(())
    })
}
