//! Compile-fail tests for the construction-lifecycle typestates
//! introduced in session T2 (Doctrine D1 -- make invalid states
//! unrepresentable). Each fixture in `tests/compile_fail/` documents
//! the runtime LLVM check it pulls forward to compile time.

/// Mirrors verifier/runtime lifecycle checks by pulling invalid construction
/// lifecycles forward into compile-fail fixtures listed below.
///
/// `t.pass(...)` is registered alongside the `compile_fail` cases so that
/// `trybuild::cargo`'s `if project.has_pass { "build" } else { "check" }`
/// switch runs the whole harness under `cargo build` instead of `cargo
/// check`. This is load-bearing for
/// `extract_value_empty_indices.rs`: its `const { assert!(N > 0) }`
/// (Doctrine D3) is a monomorphisation/codegen-time `E0080` diagnostic
/// that a `cargo check` never reaches (verified empirically: the same
/// fixture reports "succeeded" under `check` and fails correctly under
/// `build`), so without a `pass` case that fixture would silently never
/// fail to compile.
#[test]
fn typestate_compile_fail() {
    let t = trybuild::TestCases::new();
    // Relies on the unblessed `has_pass` workaround (dtolnay/trybuild#258); re-verify this still forces `cargo build` mode after any trybuild version bump.
    t.pass("tests/compile_fail/extract_value_dyn_empty_slice_compiles.rs");
    t.compile_fail("tests/compile_fail/position_at_end_terminated_block.rs");
    t.compile_fail("tests/compile_fail/retained_unterminated_block_cannot_reposition.rs");
    t.compile_fail("tests/compile_fail/terminated_block_cannot_start_cursor.rs");
    // Slice 7 "the break": the raw typed-phi builders and the open-phi
    // `add_incoming`/`finish` mutators are `pub(crate)`, so block arguments
    // (`append_block_with_params` + `build_*_with_args`) are the ONLY public
    // phi-authoring surface. This replaces the three former phi-typestate
    // fixtures (add-after-finish / retained-open / reopen-through-kind): once
    // the raw builders are unnameable, an external caller cannot even construct
    // the Open phi those fixtures needed, so the load-bearing guarantee is now
    // that the builder itself cannot be named.
    t.compile_fail("tests/compile_fail/raw_phi_builder_is_unnameable.rs");
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
    t.compile_fail("tests/compile_fail/reshape_stale_cfg_analysis_across_edit.rs");
    t.compile_fail("tests/compile_fail/atomicrmw_set_value_requires_token.rs");
    t.compile_fail("tests/compile_fail/patchbody_cannot_erase_terminator.rs");
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
    t.compile_fail("tests/compile_fail/cross_module_value_brand.rs");
    t.compile_fail("tests/compile_fail/cross_module_global_initializer_brand.rs");
    t.compile_fail("tests/compile_fail/cross_module_branch_target.rs");
    t.compile_fail("tests/compile_fail/cross_module_select_arm.rs");
    t.compile_fail("tests/compile_fail/custom_folder_wrong_brand.rs");
    t.compile_fail("tests/compile_fail/function_analysis_wrong_brand.rs");
    t.compile_fail("tests/compile_fail/module_analysis_readonly_globals.rs");
    t.compile_fail("tests/compile_fail/verified_module_core_escape.rs");
    t.compile_fail("tests/compile_fail/unverified_module_no_deref_core.rs");
    t.compile_fail("tests/compile_fail/saved_function_handle_requires_unverified_token.rs");
    t.compile_fail("tests/compile_fail/saved_global_handle_requires_unverified_token.rs");
    t.compile_fail("tests/compile_fail/intrinsic_id_raw_constructor_private.rs");
    t.compile_fail("tests/compile_fail/binary_folder_rejects_non_binary_intrinsic.rs");
    t.compile_fail("tests/compile_fail/default_pipeline_o2_not_supported.rs");
    t.compile_fail("tests/compile_fail/module_pipeline_step_rejects_raw_string.rs");
    t.compile_fail("tests/compile_fail/select_arm_forge.rs");
    t.compile_fail("tests/compile_fail/folder_typed_wrong_width.rs");
    // Slice 5: typed vector ops make element/length mismatches compile errors.
    t.compile_fail("tests/compile_fail/vec_binop_length_mismatch.rs");
    t.compile_fail("tests/compile_fail/vec_binop_element_mismatch.rs");
    t.compile_fail("tests/compile_fail/vec_insert_wrong_element.rs");
    // Slice 6: typed array ops make wrong-element inserts compile errors.
    t.compile_fail("tests/compile_fail/array_insert_wrong_element.rs");
    t.compile_fail("tests/compile_fail/typed_gep_bad_index.rs");
    t.compile_fail("tests/compile_fail/fp_ext_equal_width.rs");
    t.compile_fail("tests/compile_fail/extract_value_empty_indices.rs");
    t.compile_fail("tests/compile_fail/typed_call_wrong_arity.rs");
    t.compile_fail("tests/compile_fail/typed_call_wrong_arg_type.rs");
    t.compile_fail("tests/compile_fail/typed_call_wrong_arg_type_lifted.rs");
    t.compile_fail("tests/compile_fail/typed_call_void_result_use.rs");
    t.compile_fail("tests/compile_fail/typed_call_cross_module_arg.rs");
    t.compile_fail("tests/compile_fail/ssa_def_unpositioned.rs");
    t.compile_fail("tests/compile_fail/ssa_use_after_terminator.rs");
    t.compile_fail("tests/compile_fail/ssa_def_wrong_width.rs");
    t.compile_fail("tests/compile_fail/ssa_ret_value_in_void_fn.rs");
    t.compile_fail("tests/compile_fail/ssa_finish_positioned.rs");
    // capability-graded pass API capability-rung locks (Task 9). Each proves a rung guarantee
    // whose primary error is one of OUR OWN stable messages (an `E0599`
    // absent-method, a `#[diagnostic::on_unimplemented]`, or a `syn::Error`),
    // which do not drift across rustc versions.
    t.compile_fail("tests/compile_fail/inspect_pass_cannot_mutate.rs");
    t.compile_fail("tests/compile_fail/undeclared_analysis_in_pass_body.rs");
    t.compile_fail("tests/compile_fail/mutating_pass_cannot_enter_readonly_dyn.rs");
    t.compile_fail("tests/compile_fail/function_pass_missing_name.rs");
    t.compile_fail("tests/compile_fail/function_pass_wrong_level_access.rs");
    t.compile_fail("tests/compile_fail/claim_preserved_after_mutate.rs");
}
