//! Locks the factorial example's `.ll` output byte-for-byte.
//!
//! ## Upstream provenance
//!
//! example: locks output of `examples/factorial.rs`. Closest upstream
//! functional pattern: `unittests/IR/IRBuilderTest.cpp` (recursive /
//! looped function building via IRBuilder).
//!
//! Mirrors the `cpu_state_add_example.rs` pattern: `#[path]`-import the
//! example body, build into a fresh module, assert on the format result.

#[path = "../examples/factorial.rs"]
mod factorial_example;

use llvmkit_ir::{IrError, Module};

/// example: locks `examples/factorial.rs` output byte-for-byte.
/// Closest upstream functional reference: `unittests/IR/IRBuilderTest.cpp`
/// (loop+phi construction patterns).
#[test]
fn factorial_example_emits_locked_ir() -> Result<(), IrError> {
    // Function-pointer coercion: marks `main` as used without running it.
    let _: fn() = factorial_example::main;
    let m = Module::new("factorial");
    factorial_example::build(&m)?;
    let actual = format!("{m}");
    let expected = "; ModuleID = 'factorial'\n\
define i32 @factorial(i32 %n) {\n\
entry:\n  %is_zero = icmp eq i32 %n, 0\n  br i1 %is_zero, label %base, label %loop\n\n\
base:\n  ret i32 1\n\n\
loop:\n  %acc = phi i32 [ 1, %entry ], [ %next_acc, %loop ]\n  %i = phi i32 [ %n, %entry ], [ %next_i, %loop ]\n  %next_acc = mul i32 %acc, %i\n  %next_i = sub i32 %i, 1\n  %done = icmp eq i32 %next_i, 0\n  br i1 %done, label %exit, label %loop\n\n\
exit:\n  ret i32 %next_acc\n\
}\n";
    assert_eq!(actual, expected, "got:\n{actual}");
    Ok(())
}
