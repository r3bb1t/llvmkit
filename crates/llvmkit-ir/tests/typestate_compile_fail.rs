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
    // Cycle B (no-silent-erasure) strict cut: the erased-signature +
    // typed-return `add_function::<R>` constructor is deleted from the
    // public surface; this lock pins its absence (E0599).
    t.compile_fail("tests/compile_fail/add_function_removed.rs");
    t.compile_fail("tests/compile_fail/position_at_end_terminated_block.rs");
    // Cycle E: the *linearity* half of "one terminator per block", which the
    // fixture above does not cover. `position_at_end_terminated_block` proves a
    // `Terminated` BLOCK cannot be re-positioned into; this proves the BUILDER
    // is gone, because every terminator-emitting build takes `self` by value.
    // Primary error is rustc's stable `E0382`.
    t.compile_fail("tests/compile_fail/builder_cannot_terminate_twice.rs");
    t.compile_fail("tests/compile_fail/retained_unterminated_block_cannot_reposition.rs");
    t.compile_fail("tests/compile_fail/terminated_block_cannot_start_cursor.rs");
    // Slice 7 "the break": the raw typed-phi builders and the open-phi
    // `add_incoming`/`finish` mutators are `pub(crate)`, so block arguments
    // (`append_block_with_params` + `*_with_args`) are the ONLY public
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
    // 0.0.4 cycle D1 (`SsaBuilder` converges on the cursor model):
    // three former fixtures here — `ssa_def_unpositioned`,
    // `ssa_finish_positioned`, `ssa_use_after_terminator` — proved the SSA
    // layer's `Unpositioned`/`Positioned` type-state, which cycle D
    // *deliberately retired* in favour of a cursor held as data. Blessing their
    // `.stderr` was not an option (the code they contain now compiles), and
    // migrating their sources would have left them proving nothing, so they
    // were deleted and replaced by runtime locks on the errors that took over:
    // `unpositioned_def_is_a_typed_runtime_error`,
    // `second_terminator_on_a_finished_block_is_unpositioned` and
    // `finish_while_positioned_names_the_open_block` in `tests/ssa_builder.rs`.
    // The two fixtures below survive untouched in doctrine: neither concerns
    // positioning, and both still fail for exactly the same reason (an
    // unsatisfied `IntoIntValue<i32>` / `IntoReturnValue<()>` bound) — only the
    // `note: required by a bound in ...` line moved, since the methods now live
    // in an impl block without the `Positioned` parameter.
    t.compile_fail("tests/compile_fail/ssa_def_wrong_width.rs");
    t.compile_fail("tests/compile_fail/ssa_ret_value_in_void_fn.rs");
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
    // Cycle D slice D.2 (rung honesty, module surface): the module's
    // *declaration* capability (`&Module<Unverified>`, carrying `add_global` /
    // `add_function_dyn` / `set_struct_body`) is `pub(crate)` on the function
    // rungs, so a `PatchBody` pass cannot mutate the module structurally and
    // still report the body-level preservation floor. Type construction — which
    // only interns into the context and invalidates nothing — stays reachable
    // through the read-only `FnPatch::module` view, so the fixture proves a
    // *boundary*, not a blanket ban: its first statement compiles and its second
    // does not. Primary error is rustc's stable `E0624` privacy diagnostic.
    t.compile_fail("tests/compile_fail/function_rung_cannot_declare_globals.rs");
    // Slice 2 (typed terminator edit handles): the handle *type* fixes which
    // edge ops exist, so a structurally-invalid CFG edge edit is a compile
    // error. Each fixture's primary error is one of OUR OWN stable messages —
    // an `E0599` absent-method (no `remove_*` where an edge is not removable)
    // or an `E0382` use-after-move (a `cond_br` collapse consumes the handle) —
    // which do not drift across rustc versions.
    t.compile_fail("tests/compile_fail/invoke_edit_has_no_remove.rs");
    t.compile_fail("tests/compile_fail/callbr_edit_has_no_remove.rs");
    t.compile_fail("tests/compile_fail/uncond_br_edit_has_no_remove.rs");
    t.compile_fail("tests/compile_fail/switch_edit_has_no_remove_default.rs");
    t.compile_fail("tests/compile_fail/cond_br_edit_remove_consumes.rs");
    // BP Slice 3 (typed `BlockCall` edge): `BasicBlockLabel::call` carries a
    // `CallArgs<Params>` bound, so seeding a typed block's head-phis with the
    // wrong arity or a wrong-typed argument is a compile error. Each fixture's
    // primary error is one of OUR OWN stable messages — `CallArgs`'s
    // `#[diagnostic::on_unimplemented]` (wrong arity) or the root
    // `IntoIntValue<i32>` trait bound (wrong lifted type) — which do not drift
    // across rustc versions.
    t.compile_fail("tests/compile_fail/block_call_wrong_arity.rs");
    t.compile_fail("tests/compile_fail/block_call_wrong_arg_type.rs");
    // OP Slice 1 (typed `SwitchInst<W>`): `switch` pins the
    // condition width `W`, so `SwitchInst::add_case` carries an
    // `IntoIntValue<'ctx, W, B>` bound and a wrong-width case value is a
    // compile error. The primary error is our own `IntoIntValue<'_, i32, _>`
    // trait bound, stable across rustc versions. The `_value_handle` companion
    // passes an already-materialised `IntValue<i64>` handle (which IS `IsValue`
    // but is NOT `IntoIntValue<'_, i32, _>`) to prove the width bound
    // *specifically* — it would compile under a hypothetical `IsValue` bound.
    t.compile_fail("tests/compile_fail/switch_case_wrong_width.rs");
    t.compile_fail("tests/compile_fail/switch_case_wrong_width_value_handle.rs");
    // OP Slice 2 (typed `indirectbr` address): `indirectbr` binds the
    // address by `IntoPointerValue<'ctx, B>`, so a typed non-pointer value
    // handle (an `IntValue<i32>`) is a compile error — the pointer-ness check
    // moves from `verify()` to build/compile time. The primary error is our
    // own `IntoPointerValue` trait bound, stable across rustc versions.
    t.compile_fail("tests/compile_fail/indirectbr_non_pointer_address.rs");
    // 0.0.4 cycle A slice A4 (no-silent-erasure at operand positions):
    // the three typed value ids lift into their handle via `IntoIntValue` &c,
    // but the *erased* `ValueId` deliberately does not — erased -> typed must
    // be spelled with `try_view`, never lifted implicitly. The primary error
    // is our own unsatisfied `IntoIntValue` trait bound, stable across rustc
    // versions (`B` is pinned by the argument brand, so no incidental
    // inference failure masks it).
    t.compile_fail("tests/compile_fail/erased_id_not_int_operand.rs");
    // 0.0.4 cycle C4 (owned modules, branded by type): two *named* brands
    // separate two modules statically, so a storable id minted by one is not
    // even the right type to hand to the other's resolver. The compile-time
    // twin of `module_ownership.rs`'s stale-generation test, which locks the
    // runtime half for two generations of the *same* brand.
    t.compile_fail("tests/compile_fail/cross_named_brand_id_view.rs");
    // 0.0.4 polish freeze: the same law, now reaching the *metadata* currency.
    // A metadata handle used to be a bare arena index with neither a brand nor
    // a `ModuleId` tag, so one module's node could be attached to another and
    // silently mis-resolve. `MetadataId<B>` carries both; two named brands make
    // the mix-up a type error, and `module_ownership.rs` locks the runtime tag
    // for the same-brand / `DynBrand` case.
    t.compile_fail("tests/compile_fail/cross_module_metadata_attachment.rs");
    // W6: the same law for the *named*-metadata currency. A named-metadata
    // handle used to be a bare `usize` list index carrying neither a brand nor
    // a `ModuleId` tag; `NamedMetadataId<B>` carries both, so two named brands
    // make the mix-up a type error, and `module_ownership.rs` locks the
    // runtime tag for the same-brand / `DynBrand` case.
    t.compile_fail("tests/compile_fail/cross_module_named_metadata_id.rs");
    // Pins the premise of the `Send` compile-assert in `module_ownership.rs`:
    // the brand type used there really is `!Send`, so asserting that
    // `Module<NotSendBrand, S>: Send` is not vacuous.
    t.compile_fail("tests/compile_fail/not_send_brand_is_really_not_send.rs");
    // Bare brands (0.0.4 freeze): `#[derive(Branded)]` emits `impl Copy`
    // without inferred bounds, but the compiler still checks the fields —
    // a non-`Copy` field under the default full-six request is `E0204`,
    // never a silently wrong `Copy`.
    t.compile_fail("tests/compile_fail/branded_copy_needs_copy_fields.rs");
    // Cycle E: a module is an owned value that can be dropped, so a borrowing
    // handle minted from it cannot outlive it (`E0597`). The compile-time law
    // that makes the storable id family necessary rather than merely
    // convenient — the `.id()` form of the same program compiles, which is why
    // a stale *id* is a run-time rejection (`module_ownership.rs`) while a
    // stale *view* cannot be constructed at all.
    t.compile_fail("tests/compile_fail/view_cannot_outlive_its_module.rs");
    // Cycle E: instruction metadata was the one mutator that took no
    // `&Module<B, Unverified>` token, so a `Verified` module's printed IR could
    // be changed through a read-only `InstructionView` — and an `Inspect`-rung
    // pass, which holds only views, could rewrite `!dbg` while the driver
    // reported everything preserved. The token is now required, matching the
    // metadata setters on `FunctionValue`/`GlobalVariable` and `set_name`.
    t.compile_fail("tests/compile_fail/verified_module_metadata_is_immutable.rs");
}
