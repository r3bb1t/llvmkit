//! Compile-fail tests for the construction-lifecycle typestates
//! introduced in session T2 (Doctrine D1 -- make invalid states
//! unrepresentable). Each fixture in `tests/compile_fail/` documents
//! the runtime LLVM check it pulls forward to compile time.

/// Mirrors verifier/runtime lifecycle checks by pulling invalid construction
/// lifecycles forward into compile-fail fixtures listed below.
#[test]
fn typestate_compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/position_at_end_terminated_block.rs");
    t.compile_fail("tests/compile_fail/add_incoming_after_finish.rs");
    t.compile_fail("tests/compile_fail/retained_unterminated_block_cannot_reposition.rs");
    t.compile_fail("tests/compile_fail/terminated_block_cannot_start_cursor.rs");
    t.compile_fail("tests/compile_fail/retained_open_phi_cannot_add_after_finish.rs");
    t.compile_fail("tests/compile_fail/finished_phi_cannot_reopen_through_instruction_kind.rs");
    t.compile_fail("tests/compile_fail/retained_open_switch_cannot_add_after_finish.rs");
    t.compile_fail("tests/compile_fail/retained_open_indirectbr_cannot_add_after_finish.rs");
    t.compile_fail("tests/compile_fail/retained_open_landingpad_cannot_add_after_finish.rs");
    t.compile_fail("tests/compile_fail/retained_open_catchswitch_cannot_add_after_finish.rs");
    t.compile_fail("tests/compile_fail/finished_switch_cannot_reopen_through_terminator_kind.rs");
    t.compile_fail(
        "tests/compile_fail/finished_landingpad_cannot_reopen_through_instruction_kind.rs",
    );
    t.compile_fail("tests/compile_fail/per_opcode_handle_cannot_mint_instruction.rs");
    t.compile_fail("tests/compile_fail/value_cannot_mint_instruction_lifecycle.rs");
    t.compile_fail("tests/compile_fail/block_terminator_view_cannot_erase.rs");
    t.compile_fail("tests/compile_fail/call_void_no_return_accessor.rs");
    t.compile_fail("tests/compile_fail/typed_function_params_reject_wrong_binding.rs");
    t.compile_fail("tests/compile_fail/typed_function_params_require_facade.rs");
    t.compile_fail("tests/compile_fail/typed_function_params_token_cannot_escape.rs");
    t.compile_fail("tests/compile_fail/set_struct_body_twice.rs");
    t.compile_fail("tests/compile_fail/ir_struct_tuple_shape.rs");
    t.compile_fail("tests/compile_fail/ir_struct_generics.rs");
    t.compile_fail("tests/compile_fail/ir_struct_unknown_attribute.rs");
    t.compile_fail("tests/compile_fail/ir_struct_accessor_type_mismatch.rs");
    t.compile_fail("tests/compile_fail/ir_struct_cross_module_brand.rs");
    t.compile_fail("tests/compile_fail/ir_struct_try_from_cross_module_brand.rs");
    t.compile_fail("tests/compile_fail/module_pass_requires_verified.rs");
    t.compile_fail("tests/compile_fail/cross_module_value_brand.rs");
    t.compile_fail("tests/compile_fail/cross_module_global_initializer_brand.rs");
    t.compile_fail("tests/compile_fail/cross_module_branch_target.rs");
    t.compile_fail("tests/compile_fail/cross_module_select_arm.rs");
    t.compile_fail("tests/compile_fail/custom_folder_wrong_brand.rs");
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
    t.compile_fail("tests/compile_fail/intrinsic_id_raw_constructor_private.rs");
    t.compile_fail("tests/compile_fail/binary_folder_rejects_non_binary_intrinsic.rs");
    t.compile_fail("tests/compile_fail/select_arm_forge.rs");
    t.compile_fail("tests/compile_fail/folder_typed_wrong_width.rs");
    t.compile_fail("tests/compile_fail/typed_gep_bad_index.rs");
    t.compile_fail("tests/compile_fail/fp_ext_equal_width.rs");
}
