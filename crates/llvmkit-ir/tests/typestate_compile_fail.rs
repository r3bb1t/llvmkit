//! Compile-fail tests for the construction-lifecycle typestates
//! introduced in session T2 (Doctrine D1 -- make invalid states
//! unrepresentable). Each fixture in `tests/compile_fail/` documents
//! the runtime LLVM check it pulls forward to compile time.

#[test]
fn typestate_compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/position_at_end_sealed_block.rs");
    t.compile_fail("tests/compile_fail/add_incoming_after_finish.rs");
    t.compile_fail("tests/compile_fail/call_void_no_return_accessor.rs");
    t.compile_fail("tests/compile_fail/set_struct_body_twice.rs");
}
