//! Locks the byte-for-byte output of the `cpu_state_add` example.
//!
//! ## Upstream provenance
//!
//! example: locks output of `examples/cpu_state_add.rs`. Closest upstream
//! functional pattern: `unittests/IR/IRBuilderTest.cpp` (multi-instruction
//! function building via IRBuilder).
//!
//! The example exercises every Phase A3 + Phase D-lite + Phase C
//! `build_trunc` deliverable in one place. If this test ever diverges
//! from the example, one of them is wrong.

use llvmkit_ir::Module;

#[path = "../examples/cpu_state_add.rs"]
mod example;

/// example: locks `examples/cpu_state_add.rs` output byte-for-byte.
/// Closest upstream functional reference: `unittests/IR/IRBuilderTest.cpp`
/// (trunc + add + ret IRBuilder patterns).
#[test]
fn cpu_state_add_matches_priorities_section_byte_for_byte() {
    // Reference `main` so the dead-code lint stays quiet without an
    // `#[allow]`. Function-pointer coercion is a real use that does not
    // run the function.
    let _: fn() = example::main;
    let m = Module::new("cpu_state_add");
    example::build(&m).expect("build succeeds");
    let actual = format!("{m}");
    let expected = "\
; ModuleID = 'cpu_state_add'
define i32 @add(i64 %rax, i64 %rbx, i64 %rcx, i64 %rdx) local_unnamed_addr {
entry:
  %0 = trunc i64 %rax to i32
  %1 = trunc i64 %rbx to i32
  %2 = trunc i64 %rcx to i32
  %add1 = add i32 %0, %1
  %add2 = add i32 %add1, %2
  ret i32 %add2
}

define noundef i32 @main() local_unnamed_addr {
entry:
  ret i32 1
}
";
    assert_eq!(actual, expected, "got:\n{actual}");
}
