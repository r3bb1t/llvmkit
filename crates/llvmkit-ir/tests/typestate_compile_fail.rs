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
    t.compile_fail("tests/compile_fail/module_pass_requires_verified.rs");
    t.compile_fail("tests/compile_fail/cross_module_value_brand.rs");
    t.compile_fail("tests/compile_fail/function_analysis_wrong_brand.rs");
    t.compile_fail("tests/compile_fail/unverified_output_requires_verify.rs");
    t.compile_fail("tests/compile_fail/function_pass_no_mutable_module_manager.rs");
    t.compile_fail("tests/compile_fail/function_pass_requires_verified.rs");
    t.compile_fail("tests/compile_fail/module_analysis_readonly_globals.rs");
    t.compile_fail("tests/compile_fail/verified_module_core_escape.rs");
    t.compile_fail("tests/compile_fail/unverified_module_no_deref_core.rs");
    t.compile_fail("tests/compile_fail/saved_function_handle_requires_unverified_token.rs");
    t.compile_fail("tests/compile_fail/saved_global_handle_requires_unverified_token.rs");
    t.compile_fail("tests/compile_fail/read_only_pass_manager_rejects_transform_pass.rs");
    t.compile_fail("tests/compile_fail/transform_pass_manager_output_requires_verify.rs");
}
