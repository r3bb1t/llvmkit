# Upstream Test Provenance Registry

Per Doctrine D11 (see `local://RLLVM_TYPE_SAFETY_SWEEP.md`), every llvmkit test
cites the upstream LLVM test, fixture, or reference it ports. When the
upstream tree has no equivalent (a typestate compile-fail that LLVM checks
at runtime, or an AsmWriter byte-for-byte parity check), the row is
marked `llvmkit-specific` with the closest functional reference.

Categories:
- `port` --- direct port of an upstream `TEST(...)` / `TEST_F(...)`.
- `mirror` --- lifts an upstream `.ll` fixture or rule shape.
- `example` --- locks output of an `examples/*.rs` binary.
- `llvmkit-specific` --- llvmkit-only test (typestate compile-fail, format-stability,
  Rust-API ergonomics) with the closest upstream functional reference cited.

Reference root: `orig_cpp/llvm-project-llvmorg-22.1.4/llvm/`.

Total `#[test]` functions: 367.

| llvmkit test | upstream reference | category |
|---|---|---|
| `crates/llvmkit-ir/tests/asm_writer_basic.rs::module_prints_simple_add_function` | `unittests/IR/AsmWriterTest.cpp::TEST(AsmWriterTest, DebugPrintDetachedInstruction)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/asm_writer_basic.rs::module_prints_const_folded_arithmetic` | `unittests/IR/AsmWriterTest.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/tests/asm_writer_basic.rs::function_print_standalone_matches_module_section` | `unittests/IR/AsmWriterTest.cpp::TEST(AsmWriterTest, DebugPrintDetachedInstruction)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/asm_writer_basic.rs::declare_form_for_empty_function` | `unittests/IR/AsmWriterTest.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/tests/asm_writer_basic.rs::unnamed_basic_block_uses_slot_label` | `unittests/IR/AsmWriterTest.cpp::TEST(AsmWriterTest, DebugPrintDetachedArgument)` | mirror |
| `crates/llvmkit-ir/tests/builder_alloca.rs::alloca_plain` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, Lifetime)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/builder_alloca.rs::alloca_array_size` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, Lifetime)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/builder_alloca.rs::alloca_aligned` | `test/Assembler/align-inst-alloca.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_call.rs::call_int_returning_function` | `unittests/IR/InstructionsTest.cpp::TEST_F(ModuleWithFunctionTest, CallInst)` | port |
| `crates/llvmkit-ir/tests/builder_call.rs::call_void_returning_function` | `unittests/IR/InstructionsTest.cpp::TEST_F(ModuleWithFunctionTest, CallInst)` | port |
| `crates/llvmkit-ir/tests/builder_call.rs::call_builder_mixed_arg_types` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CloneCall)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/builder_call.rs::call_tail` | `unittests/IR/InstructionsTest.cpp::TEST_F(ModuleWithFunctionTest, CallInst)` | mirror |
| `crates/llvmkit-ir/tests/builder_call.rs::call_to_pointer_returning_function` | `unittests/IR/InstructionsTest.cpp::TEST_F(ModuleWithFunctionTest, CallInst)` | port |
| `crates/llvmkit-ir/tests/builder_cast_fp.rs::fpext_f32_to_f64` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)` | port |
| `crates/llvmkit-ir/tests/builder_cast_fp.rs::fptrunc_f64_to_f32` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)` | port |
| `crates/llvmkit-ir/tests/builder_cast_fp.rs::fptosi_f32_to_i32` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)` | port |
| `crates/llvmkit-ir/tests/builder_cast_fp.rs::fptoui_f32_to_i32` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)` | port |
| `crates/llvmkit-ir/tests/builder_cast_fp.rs::sitofp_i32_to_f32` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)` | port |
| `crates/llvmkit-ir/tests/builder_cast_fp.rs::uitofp_i32_to_f32` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)` | port |
| `crates/llvmkit-ir/tests/builder_cast_ptr_int.rs::ptrtoint_emits_canonical_form` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)` | port |
| `crates/llvmkit-ir/tests/builder_cast_ptr_int.rs::inttoptr_emits_canonical_form` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)` | port |
| `crates/llvmkit-ir/tests/builder_cast_ptr_int.rs::addrspacecast_emits_canonical_form` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)` | mirror |
| `crates/llvmkit-ir/tests/builder_fp_arith.rs::fadd_f32` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)` | port |
| `crates/llvmkit-ir/tests/builder_fp_arith.rs::fsub_f32` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)` | port |
| `crates/llvmkit-ir/tests/builder_fp_arith.rs::fmul_f32` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)` | port |
| `crates/llvmkit-ir/tests/builder_fp_arith.rs::fdiv_f32` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)` | port |
| `crates/llvmkit-ir/tests/builder_fp_arith.rs::frem_f32` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)` | port |
| `crates/llvmkit-ir/tests/builder_fp_arith.rs::fadd_f64` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)` | port |
| `crates/llvmkit-ir/tests/builder_fp_cmp.rs::fcmp_oeq` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/builder_fp_cmp.rs::fcmp_ogt` | `-` | llvmkit-specific |
| `crates/llvmkit-ir/tests/builder_fp_cmp.rs::fcmp_olt` | `-` | llvmkit-specific |
| `crates/llvmkit-ir/tests/builder_fp_cmp.rs::fcmp_ord` | `-` | llvmkit-specific |
| `crates/llvmkit-ir/tests/builder_fp_cmp.rs::fcmp_une` | `-` | llvmkit-specific |
| `crates/llvmkit-ir/tests/builder_fp_cmp.rs::fcmp_uno` | `-` | llvmkit-specific |
| `crates/llvmkit-ir/tests/builder_gep.rs::gep_array_offset` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, GEPIndices)` | port |
| `crates/llvmkit-ir/tests/builder_gep.rs::gep_inbounds` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, GEPIndices)` | port |
| `crates/llvmkit-ir/tests/builder_gep.rs::struct_gep` | `test/Assembler/getelementptr_struct.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_gep.rs::gep_zero_index` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, ZeroIndexGEP)` | port |
| `crates/llvmkit-ir/tests/builder_int_div_rem.rs::udiv_plain` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, WrapFlags)` | mirror |
| `crates/llvmkit-ir/tests/builder_int_div_rem.rs::sdiv_plain` | `test/Assembler/flags.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_int_div_rem.rs::urem_plain` | `test/Assembler/flags.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_int_div_rem.rs::srem_plain` | `test/Assembler/flags.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_int_div_rem.rs::udiv_exact` | `test/Assembler/flags.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_int_div_rem.rs::sdiv_exact` | `test/Assembler/flags.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_int_logical.rs::and_print_form` | `test/Assembler/flags.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_int_logical.rs::or_print_form` | `test/Assembler/flags.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_int_logical.rs::xor_print_form` | `test/Assembler/flags.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_int_shifts.rs::shl_plain` | `test/Assembler/flags.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_int_shifts.rs::lshr_plain` | `test/Assembler/flags.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_int_shifts.rs::ashr_plain` | `test/Assembler/flags.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_int_shifts.rs::shl_nuw_nsw` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, WrapFlags)` | port |
| `crates/llvmkit-ir/tests/builder_int_shifts.rs::lshr_exact` | `test/Assembler/flags.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_int_shifts.rs::ashr_exact` | `test/Assembler/flags.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_load_store.rs::load_plain` | `test/Assembler/align-inst-load.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_load_store.rs::load_aligned` | `test/Assembler/align-inst-load.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_load_store.rs::store_plain` | `test/Assembler/align-inst-store.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_load_store.rs::store_aligned` | `test/Assembler/align-inst-store.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_load_store.rs::load_add_store_round_trip` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/builder_load_store.rs::align_invariant` | `lib/Support/Alignment.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/tests/builder_select.rs::select_int_arms` | `test/Assembler/select.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_select.rs::select_fp_arms` | `test/Assembler/select.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_select.rs::select_pointer_arms` | `test/Assembler/select.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_wrap_flags.rs::add_nuw_nsw_flags_round_trip` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, WrapFlags)` | port |
| `crates/llvmkit-ir/tests/builder_wrap_flags.rs::sub_mul_shl_flags_round_trip` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, WrapFlags)` | port |
| `crates/llvmkit-ir/tests/builder_wrap_flags.rs::div_shr_exact_round_trip` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, WrapFlags)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/cpu_state_add_example.rs::cpu_state_add_matches_priorities_section_byte_for_byte` | `unittests/IR/IRBuilderTest.cpp` | example |
| `crates/llvmkit-ir/tests/custom_width.rs::int_type_n_constructor_matches_upstream_int3` | `-` | mirror |
| `crates/llvmkit-ir/tests/custom_width.rs::width_marker_works_as_return_marker` | `-` | llvmkit-specific |
| `crates/llvmkit-ir/tests/factorial_example.rs::factorial_example_emits_locked_ir` | `unittests/IR/IRBuilderTest.cpp` | example |
| `crates/llvmkit-ir/tests/medium_builder_cast.rs::build_trunc_emits_trunc_to_dst_type` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)` | mirror |
| `crates/llvmkit-ir/tests/medium_builder_cast.rs::build_trunc_dyn_runtime_check_widening_rejected` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/medium_builder_cast.rs::build_trunc_preserves_anonymous_slot_naming` | `unittests/IR/AsmWriterTest.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/tests/medium_builder_cast.rs::build_zext_static_static_emits_zext` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)` | mirror |
| `crates/llvmkit-ir/tests/medium_builder_cast.rs::build_sext_static_static_emits_sext` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)` | mirror |
| `crates/llvmkit-ir/tests/medium_builder_cmp.rs::build_int_cmp_eq_emits_icmp_eq` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CmpPredicate)` | mirror |
| `crates/llvmkit-ir/tests/medium_builder_cmp.rs::build_int_cmp_slt_emits_icmp_slt` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CmpPredicate)` | mirror |
| `crates/llvmkit-ir/tests/medium_builder_cmp.rs::build_int_cmp_returns_i1_for_chaining` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CmpPredicate)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/medium_builder_cmp.rs::build_int_cmp_ule_emits_icmp_ule` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CmpPredicate)` | mirror |
| `crates/llvmkit-ir/tests/medium_builder_control_flow.rs::build_br_emits_unconditional` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)` | mirror |
| `crates/llvmkit-ir/tests/medium_builder_control_flow.rs::build_cond_br_branches_on_i1` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)` | port |
| `crates/llvmkit-ir/tests/medium_builder_control_flow.rs::build_unreachable_terminator` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/medium_builder_int.rs::build_int_add_accepts_int_value_and_rust_literal` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, NoFolderNames)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/medium_builder_int.rs::build_int_sub_accepts_constant_and_argument` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, NoFolderNames)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/medium_builder_int.rs::build_ret_accepts_rust_literal_directly` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, NoFolderNames)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/medium_builder_phi.rs::build_int_phi_two_predecessors_emits_phi` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)` | mirror |
| `crates/llvmkit-ir/tests/medium_builder_phi.rs::phi_with_post_creation_add_incoming` | `unittests/IR/BasicBlockTest.cpp::TEST(BasicBlockTest, PhiRange)` | mirror |
| `crates/llvmkit-ir/tests/mutation_basic.rs::use_test_sort_setup_registers_eight_users` | `unittests/IR/UseTest.cpp::TEST(UseTest, sort)` | port |
| `crates/llvmkit-ir/tests/mutation_basic.rs::erase_no_invalidation` | `unittests/IR/BasicBlockTest.cpp::TEST_F(InstrOrderInvalidationTest, EraseNoInvalidation)` | port |
| `crates/llvmkit-ir/tests/mutation_basic.rs::erase_deregisters_from_operand_use_lists` | `-` | llvmkit-specific |
| `crates/llvmkit-ir/tests/parameter_attributes.rs::function_with_noundef_param_and_return` | `unittests/IR/AttributesTest.cpp::TEST(Attributes, AddAttributes)` | mirror |
| `crates/llvmkit-ir/tests/parameter_attributes.rs::attribute_added_via_attribute_method_path` | `unittests/IR/AttributesTest.cpp::TEST(Attributes, AddAttributes)` | mirror |
| `crates/llvmkit-ir/tests/phase_a_types.rs::primitive_types_intern_to_same_id` | `unittests/IR/TypesTest.cpp::TEST(TypesTest, LayoutIdenticalEmptyStructs)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/phase_a_types.rs::integer_widths_distinct` | `unittests/IR/TypesTest.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/tests/phase_a_types.rs::integer_width_validation` | `lib/IR/Type.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/tests/phase_a_types.rs::array_and_vector_intern` | `unittests/IR/VectorTypesTest.cpp::TEST(VectorTypesTest, FixedLength)` | port |
| `crates/llvmkit-ir/tests/phase_a_types.rs::function_type_round_trip` | `unittests/IR/TypesTest.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/tests/phase_a_types.rs::literal_struct_intern` | `unittests/IR/TypesTest.cpp::TEST(TypesTest, LayoutIdenticalEmptyStructs)` | port |
| `crates/llvmkit-ir/tests/phase_a_types.rs::named_struct_forward_decl_then_set_body` | `unittests/IR/TypesTest.cpp::TEST(TypesTest, StructType)` | port |
| `crates/llvmkit-ir/tests/phase_a_types.rs::missing_named_struct_returns_none` | `lib/IR/Module.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/tests/phase_a_types.rs::type_kind_discriminator_is_correct` | `include/llvm/IR/Type.h` | llvmkit-specific |
| `crates/llvmkit-ir/tests/phase_a_types.rs::sized_refinement_accepts_sized_rejects_unsized` | `lib/IR/Type.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/tests/phase_a_types.rs::first_class_predicate_rejects_function_void_opaque` | `lib/IR/Type.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/tests/phase_a_types.rs::try_from_narrows_correctly` | `-` | llvmkit-specific |
| `crates/llvmkit-ir/tests/phase_a_types.rs::basic_type_enum_classifies_first_class` | `lib/IR/Type.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/tests/phase_a_types.rs::aggregate_excludes_vector` | `include/llvm/IR/Type.h` | llvmkit-specific |
| `crates/llvmkit-ir/tests/phase_a_types.rs::any_type_enum_widens_every_kind` | `-` | llvmkit-specific |
| `crates/llvmkit-ir/tests/phase_a_types.rs::handles_implement_hash_and_eq_via_derive` | `-` | llvmkit-specific |
| `crates/llvmkit-ir/tests/phase_a_types.rs::cross_module_handles_compare_unequal` | `-` | llvmkit-specific |
| `crates/llvmkit-ir/tests/phase_a_types.rs::ir_type_trait_unifies_handles` | `-` | llvmkit-specific |
| `crates/llvmkit-ir/tests/unnamed_addr.rs::default_emits_no_unnamed_addr_token` | `test/Assembler/unnamed-addr.ll` | mirror |
| `crates/llvmkit-ir/tests/unnamed_addr.rs::local_emits_local_unnamed_addr` | `test/Assembler/local-unnamed-addr.ll` | mirror |
| `crates/llvmkit-ir/tests/unnamed_addr.rs::global_emits_unnamed_addr` | `test/Assembler/unnamed-addr.ll` | mirror |
| `crates/llvmkit-ir/tests/verifier_basic.rs::verify_empty_module` | `unittests/IR/VerifierTest.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/tests/verifier_basic.rs::verify_identity_function` | `unittests/IR/VerifierTest.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/tests/verifier_basic.rs::verify_int_arithmetic_full` | `unittests/IR/VerifierTest.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/tests/verifier_basic.rs::verify_float_arithmetic_full` | `unittests/IR/VerifierTest.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/tests/verifier_basic.rs::verify_casts_full` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, CastInst)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/verifier_basic.rs::verify_memory_gep_select_control` | `unittests/IR/VerifierTest.cpp::TEST(VerifierTest, GetElementPtrInst)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/verifier_basic.rs::verify_call` | `unittests/IR/VerifierTest.cpp::TEST(VerifierTest, CrossFunctionRef)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/verifier_basic.rs::verify_void_return_and_unreachable` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/verifier_basic.rs::verify_consuming_returns_branded_module` | `unittests/IR/VerifierTest.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/tests/verifier_basic.rs::verifier_rule_matchable` | `unittests/IR/VerifierTest.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/tests/verifier_basic.rs::verify_function_with_empty_block_fails_missing_terminator` | `unittests/IR/VerifierTest.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/tests/vertical_slice.rs::vertical_slice_compiles_and_runs` | `unittests/IR/IRBuilderTest.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/tests/vertical_slice.rs::mismatched_widths_error_at_runtime_when_dyn` | `-` | llvmkit-specific |
| `crates/llvmkit-ir/tests/vertical_slice.rs::const_int_interns` | `unittests/IR/ConstantsTest.cpp::TEST(ConstantsTest, Integer_i1)` | mirror |
| `crates/llvmkit-ir/tests/vertical_slice.rs::argument_to_int_value_narrowing_validates_type` | `-` | llvmkit-specific |
| `crates/llvmkit-ir/tests/vertical_slice.rs::duplicate_function_name_errors` | `lib/IR/Module.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/tests/vertical_slice.rs::function_builder_chains_options` | `unittests/IR/AttributesTest.cpp::TEST(Attributes, AddAttributes)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/vertical_slice.rs::typed_add_function_rejects_mismatched_return_marker` | `-` | llvmkit-specific |
| `crates/llvmkit-ir/tests/vertical_slice.rs::dyn_path_keeps_runtime_return_check` | `-` | llvmkit-specific |
| `crates/llvmkit-asmparser/tests/lexer_integration.rs::three_input_paths_yield_identical_streams` | `lib/AsmParser/LLLexer.cpp::LLLexer::LLLexer` | llvmkit-specific |
| `crates/llvmkit-asmparser/tests/lexer_integration.rs::snapshot_landmark_tokens` | `lib/AsmParser/LLLexer.cpp::LLLexer::LexToken` | llvmkit-specific |
| `crates/llvmkit-asmparser/tests/lexer_integration.rs::lex_error_propagates_via_question_mark` | `lib/AsmParser/LLLexer.cpp::LLLexer::LexQuote` | llvmkit-specific |
| `crates/llvmkit-asmparser/tests/lexer_integration.rs::cow_borrows_when_possible` | `lib/AsmParser/LLLexer.cpp::LLLexer::LexIdentifier` | llvmkit-specific |
| `crates/llvmkit-ir/src/align.rs::align_round_trip` | `include/llvm/Support/Alignment.h` | mirror |
| `crates/llvmkit-ir/src/align.rs::align_rejects_zero` | `include/llvm/Support/Alignment.h` | mirror |
| `crates/llvmkit-ir/src/align.rs::align_rejects_non_power_of_two` | `include/llvm/Support/Alignment.h::Align` | mirror |
| `crates/llvmkit-ir/src/align.rs::maybe_align_default_is_none` | `include/llvm/Support/Alignment.h` | mirror |
| `crates/llvmkit-ir/src/attributes.rs::enum_kind_constructors_validate` | `unittests/IR/AttributesTest.cpp::TEST(Attributes, AttributeRoundTrip)` | mirror |
| `crates/llvmkit-ir/src/attributes.rs::kind_partition_is_total` | `lib/IR/Attributes.cpp` | llvmkit-specific |
| `crates/llvmkit-ir/src/attributes.rs::display_renders_attribute_text` | `test/Assembler/unnamed_addr.ll` | mirror |
| `crates/llvmkit-ir/src/attributes.rs::attribute_set_dedupes_and_iterates` | `unittests/IR/AttributesTest.cpp` | mirror |
| `crates/llvmkit-ir/src/attributes.rs::attribute_list_indexed_storage` | `lib/IR/Attributes.cpp` | mirror |
| `crates/llvmkit-ir/src/attribute_mask.rs::add_and_query_kinds` | `include/llvm/IR/AttributeMask.h` | mirror |
| `crates/llvmkit-ir/src/attribute_mask.rs::add_set_collects_kinds_and_strings` | `include/llvm/IR/AttributeMask.h` | mirror |
| `crates/llvmkit-ir/src/attribute_mask.rs::contains_dispatches_by_attr_shape` | `include/llvm/IR/AttributeMask.h` | mirror |
| `crates/llvmkit-ir/src/calling_conv.rs::defaults_to_c` | `include/llvm/IR/CallingConv.h` | mirror |
| `crates/llvmkit-ir/src/calling_conv.rs::round_trip_known` | `include/llvm/IR/CallingConv.h` | llvmkit-specific |
| `crates/llvmkit-ir/src/calling_conv.rs::rejects_out_of_range` | `include/llvm/IR/CallingConv.h` | mirror |
| `crates/llvmkit-ir/src/calling_conv.rs::callable_partition` | `include/llvm/IR/CallingConv.h` | mirror |
| `crates/llvmkit-ir/src/calling_conv.rs::display_named_and_numeric` | `lib/IR/AsmWriter.cpp` | mirror |
| `crates/llvmkit-ir/src/calling_conv.rs::display_riscv_vls_parameterised` | `lib/IR/AsmWriter.cpp::PrintCallingConv` | mirror |
| `crates/llvmkit-ir/src/calling_conv.rs::unsupported_named_falls_back_to_numeric` | `lib/IR/AsmWriter.cpp::PrintCallingConv` | mirror |
| `crates/llvmkit-ir/src/cmp_predicate.rs::float_round_trip` | `include/llvm/IR/InstrTypes.h` | llvmkit-specific |
| `crates/llvmkit-ir/src/cmp_predicate.rs::int_round_trip` | `include/llvm/IR/InstrTypes.h` | llvmkit-specific |
| `crates/llvmkit-ir/src/cmp_predicate.rs::float_inverse_is_xor_15` | `lib/IR/Instructions.cpp` | mirror |
| `crates/llvmkit-ir/src/cmp_predicate.rs::int_inverse_involutive` | `lib/IR/Instructions.cpp` | mirror |
| `crates/llvmkit-ir/src/cmp_predicate.rs::int_swapped_involutive` | `lib/IR/Instructions.cpp` | mirror |
| `crates/llvmkit-ir/src/cmp_predicate.rs::float_swapped_involutive` | `lib/IR/Instructions.cpp` | mirror |
| `crates/llvmkit-ir/src/cmp_predicate.rs::int_signedness_partition` | `lib/IR/Instructions.cpp` | mirror |
| `crates/llvmkit-ir/src/cmp_predicate.rs::display_matches_llvm` | `lib/IR/Instructions.cpp` | mirror |
| `crates/llvmkit-ir/src/cmp_predicate.rs::from_raw_rejects_out_of_range` | `include/llvm/IR/InstrTypes.h` | llvmkit-specific |
| `crates/llvmkit-ir/src/fmf.rs::fast_is_all` | `include/llvm/IR/FMF.h` | mirror |
| `crates/llvmkit-ir/src/fmf.rs::display_fast` | `lib/IR/AsmWriter.cpp` | mirror |
| `crates/llvmkit-ir/src/fmf.rs::display_partial` | `lib/IR/AsmWriter.cpp` | mirror |
| `crates/llvmkit-ir/src/fmf.rs::display_empty` | `lib/IR/AsmWriter.cpp` | mirror |
| `crates/llvmkit-ir/src/fmf.rs::intersect_rewrite_drops_value_bits` | `include/llvm/IR/FMF.h` | mirror |
| `crates/llvmkit-ir/src/fmf.rs::union_value_drops_rewrite_bits` | `include/llvm/IR/FMF.h` | mirror |
| `crates/llvmkit-ir/src/gep_no_wrap_flags.rs::inbounds_implies_nusw` | `include/llvm/IR/GEPNoWrapFlags.h` | mirror |
| `crates/llvmkit-ir/src/gep_no_wrap_flags.rs::intersect_offset_add_drops_orphan_nusw` | `include/llvm/IR/GEPNoWrapFlags.h` | mirror |
| `crates/llvmkit-ir/src/gep_no_wrap_flags.rs::intersect_reassociate_requires_nuw` | `include/llvm/IR/GEPNoWrapFlags.h` | mirror |
| `crates/llvmkit-ir/src/gep_no_wrap_flags.rs::display_inbounds_hides_nusw` | `lib/IR/AsmWriter.cpp` | mirror |
| `crates/llvmkit-ir/src/gep_no_wrap_flags.rs::display_nusw_only` | `lib/IR/AsmWriter.cpp` | mirror |
| `crates/llvmkit-ir/src/gep_no_wrap_flags.rs::display_all` | `lib/IR/AsmWriter.cpp` | mirror |
| `crates/llvmkit-ir/src/verifier.rs::ret_type_mismatch_ptr_in_i32_function` | `test/Verifier/2002-04-13-RetTypes.ll` | llvmkit-specific |
| `crates/llvmkit-ir/src/verifier.rs::ret_value_in_void_function` | `test/Verifier/2008-11-15-RetVoid.ll` | llvmkit-specific |
| `crates/llvmkit-ir/src/verifier.rs::binary_operand_type_mismatch` | `-` | llvmkit-specific |
| `crates/llvmkit-ir/src/verifier.rs::br_condition_not_i1` | `-` | llvmkit-specific |
| `crates/llvmkit-ir/src/verifier.rs::misplaced_terminator` | `-` | llvmkit-specific |
| `crates/llvmkit-ir/src/verifier.rs::phi_not_at_top` | `test/Verifier/PhiGrouping.ll` | llvmkit-specific |
| `crates/llvmkit-ir/src/verifier.rs::self_reference_in_non_phi` | `test/Verifier/SelfReferential.ll` | llvmkit-specific |
| `crates/llvmkit-ir/src/verifier.rs::ambiguous_phi_duplicate_predecessor` | `test/Verifier/AmbiguousPhi.ll` | llvmkit-specific |
| `crates/llvmkit-ir/src/verifier.rs::phi_predecessor_mismatch` | `-` | llvmkit-specific |
| `crates/llvmkit-ir/src/verifier.rs::call_arg_count_mismatch` | `-` | llvmkit-specific |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::every_punctuation` | `lib/AsmParser/LLLexer.cpp::LexToken` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::dotdotdot` | `lib/AsmParser/LLLexer.cpp::LexToken` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::span_of_single_char_punct` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::global_unquoted_borrows` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::global_quoted_no_escape_borrows` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::global_quoted_with_escape_owns` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::global_id` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::mangling_prefix_decodes_to_01` | `test/Assembler/unnamed_addr.ll` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::local_unquoted_id_and_quoted` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::comdat_var` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::metadata_var_and_alone` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::metadata_var_decodes_escape` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::summary_id` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::attr_grp_and_lone_hash` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::nul_in_quoted_name_is_error` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::ident_label` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::quoted_label` | `lib/AsmParser/LLLexer.cpp::LexQuote` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::numeric_label` | `lib/AsmParser/LLLexer.cpp::LexDigitOrNegative` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::negative_label` | `lib/AsmParser/LLLexer.cpp::LexDigitOrNegative` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::ignore_colon_in_idents_suppresses_label` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | llvmkit-specific |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::primitive_types` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::integer_types_basic` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::integer_type_at_max` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::integer_type_overflow_errors` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::i_alone_is_unknown` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::decimal_int` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::hex_apsint_signed_unsigned` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::hex_double` | `lib/AsmParser/LLLexer.cpp::LexDigitOrNegative` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::hex_x87_quad_ppc` | `lib/AsmParser/LLLexer.cpp::LexDigitOrNegative` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::hex_half_and_bfloat` | `lib/AsmParser/LLLexer.cpp::LexDigitOrNegative` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::hex_half_overflow_errors` | `lib/AsmParser/LLLexer.cpp::LexDigitOrNegative` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::hex_bfloat_overflow_errors` | `lib/AsmParser/LLLexer.cpp::LexDigitOrNegative` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::fp_decimal_borrows_full_lexeme` | `lib/AsmParser/LLLexer.cpp::LexDigitOrNegative` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::plus_without_digit_errors` | `lib/AsmParser/LLLexer.cpp::LexPositive` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::structural_keywords` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::instructions` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::flags_and_attrs` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::attributes_keyword` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::cc_with_digits_rewinds` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::dwarf_tag` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::dwarf_op` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::diflag_and_dispflag` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::checksum_kind` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::dbg_record_type_strips_prefix` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::emission_kind` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::name_table_and_fixed_point` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::string_constant_borrows` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::string_constant_owns_with_escape` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::nul_in_string_is_allowed` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::newline_inside_string_does_not_terminate` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::unterminated_string_errors` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::quote_followed_by_colon_is_label` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::line_comment_consumed` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::block_comment_consumed` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::unterminated_block_comment_errors` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::slash_without_star_errors` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::unknown_token_for_question_mark` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::id_overflow_errors` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::lex_error_carries_span` | `lib/AsmParser/LLLexer.cpp` | llvmkit-specific |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::no_escape_borrows` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::escape_owns` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::nul_byte_is_whitespace` | `-` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::crlf_handled` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::empty_input_is_eof` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::integer_lit_span_excludes_following_whitespace` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::keyword_span_matches_keyword` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::quoted_global_span_includes_sigil_and_quotes` | `lib/AsmParser/LLLexer.cpp` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer_tests.rs::label_span_includes_colon` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer/escape.rs::no_escapes_borrows` | `lib/AsmParser/LLLexer.cpp::UnEscapeLexed` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer/escape.rs::double_backslash_collapses` | `lib/AsmParser/LLLexer.cpp::UnEscapeLexed` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer/escape.rs::hex_escape_decodes` | `lib/AsmParser/LLLexer.cpp::UnEscapeLexed` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer/escape.rs::nul_byte_escape` | `lib/AsmParser/LLLexer.cpp::UnEscapeLexed` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer/escape.rs::mangling_prefix_decodes` | `test/Assembler/unnamed_addr.ll` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer/escape.rs::lenient_keeps_bad_backslash_at_eof` | `lib/AsmParser/LLLexer.cpp::UnEscapeLexed` | llvmkit-specific |
| `crates/llvmkit-asmparser/src/ll_lexer/escape.rs::lenient_keeps_bad_hex` | `lib/AsmParser/LLLexer.cpp::UnEscapeLexed` | llvmkit-specific |
| `crates/llvmkit-asmparser/src/ll_lexer/escape.rs::lenient_keeps_one_hex_at_eof` | `lib/AsmParser/LLLexer.cpp::UnEscapeLexed` | llvmkit-specific |
| `crates/llvmkit-asmparser/src/ll_lexer/escape.rs::empty_input_borrows` | `lib/AsmParser/LLLexer.cpp::UnEscapeLexed` | llvmkit-specific |
| `crates/llvmkit-asmparser/src/ll_lexer/escape.rs::mixed_escapes_and_text` | `lib/AsmParser/LLLexer.cpp::UnEscapeLexed` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer/keywords.rs::types_classified` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer/keywords.rs::opcodes_classified` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer/keywords.rs::plain_keywords_classified` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer/keywords.rs::attributes_classified` | `test/Assembler/unnamed_addr.ll` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer/keywords.rs::summary_camelcase_distinct_from_snake` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-asmparser/src/ll_lexer/keywords.rs::unknown_returns_none` | `lib/AsmParser/LLLexer.cpp::LexIdentifier` | mirror |
| `crates/llvmkit-support/src/source_map.rs::line_col_basic` | `-` | llvmkit-specific |
| `crates/llvmkit-support/src/source_map.rs::line_col_eof_clamps` | `-` | llvmkit-specific |
| `crates/llvmkit-support/src/source_map.rs::line_text_trims_newlines` | `-` | llvmkit-specific |
| `crates/llvmkit-support/src/source_map.rs::empty_source` | `-` | llvmkit-specific |
| `crates/llvmkit-support/src/span.rs::span_basics` | `-` | llvmkit-specific |
| `crates/llvmkit-support/src/span.rs::span_indexes_slice` | `-` | llvmkit-specific |
| `crates/llvmkit-support/src/span.rs::spanned_map_preserves_span` | `-` | llvmkit-specific |
| `crates/llvmkit-support/src/span.rs::spanned_as_ref_borrows` | `-` | llvmkit-specific |

| `crates/llvmkit-ir/tests/builder_typestate_seal.rs::cond_br_terminator_seals_block` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)` | port |
| `crates/llvmkit-ir/tests/builder_typestate_seal.rs::phi_range_iterates_three_phis` | `unittests/IR/BasicBlockTest.cpp::TEST(BasicBlockTest, PhiRange)` | port |
| `crates/llvmkit-ir/tests/builder_typestate_seal.rs::seal_typestate_does_not_change_asm_output` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/builder_typestate_phi.rs::phi_finishes_after_all_incomings` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, CreateCondBr)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/typestate_compile_fail.rs::typestate_compile_fail` | `lib/IR/Verifier.cpp::visitBasicBlock` + `visitPHINode` (runtime forms) | llvmkit-specific |
| `crates/llvmkit-ir/tests/struct_typestate.rs::named_struct_retains_name` | `unittests/IR/TypesTest.cpp::TEST(TypesTest, StructType)` | port |
| `crates/llvmkit-ir/tests/struct_typestate.rs::opaque_to_body_set_transition` | `unittests/IR/TypesTest.cpp::TEST(TypesTest, LayoutIdenticalEmptyStructs)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/struct_typestate.rs::double_set_body_runtime_path_rejects` | `unittests/IR/TypesTest.cpp::TEST(TypesTest, StructType)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/constant_int_signs.rs::int_signs_i8_round_trips` | `unittests/IR/ConstantsTest.cpp::TEST(ConstantsTest, IntSigns)` | port |
| `crates/llvmkit-ir/tests/constant_int_signs.rs::int_signs_i32_propagates_sign` | `unittests/IR/ConstantsTest.cpp::TEST(ConstantsTest, IntSigns)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/constant_int_signs.rs::int_signs_i1_sign_extends_to_minus_one` | `unittests/IR/ConstantsTest.cpp::TEST(ConstantsTest, IntSigns)` | llvmkit-specific |
| `crates/llvmkit-ir/tests/builder_icmp_named.rs::build_icmp_eq_emits_icmp_eq` | `test/Assembler/2007-03-18-InvalidNumberedVar.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_icmp_named.rs::build_icmp_ne_emits_icmp_ne` | `test/Assembler/auto_upgrade_nvvm_intrinsics.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_icmp_named.rs::build_icmp_slt_emits_icmp_slt` | `test/Assembler/2004-02-27-SelfUseAssertError.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_icmp_named.rs::build_icmp_sge_emits_icmp_sge` | `test/Assembler/auto_upgrade_nvvm_intrinsics.ll` | mirror |

<!-- Parser-1: Session 1 instruction-set completion -->
| `crates/llvmkit-ir/tests/builder_unary_ops.rs::build_fneg_round_trip` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, UnaryOperators)` | port |
| `crates/llvmkit-ir/tests/builder_unary_ops.rs::fneg_with_fmf_prints_canonical_form` | `test/Bitcode/compatibility.ll::fastmathflags_unop` | mirror |
| `crates/llvmkit-ir/tests/builder_unary_ops.rs::fneg_double_no_flags_unnamed_result` | `test/Bitcode/compatibility.ll::instructions.unops` | mirror |
| `crates/llvmkit-ir/tests/builder_unary_ops.rs::freeze_i8_round_trip` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, FreezeInst)` | port |
| `crates/llvmkit-ir/tests/builder_unary_ops.rs::freeze_int_and_pointer_print_forms` | `test/Bitcode/compatibility.ll` lines 1732-1741 | mirror |
| `crates/llvmkit-ir/tests/builder_unary_ops.rs::verifier_accepts_freeze_int` | `unittests/IR/VerifierTest.cpp::TEST(VerifierTest, Freeze)` | port |
| `crates/llvmkit-ir/tests/builder_unary_ops.rs::va_arg_int_round_trip` | `test/Bitcode/variableArgumentIntrinsic.3.2.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_unary_ops.rs::va_arg_print_keyword_and_destination_type` | `test/Bitcode/compatibility.ll` line 1815 | mirror |
| `crates/llvmkit-ir/tests/builder_unary_ops.rs::verifier_accepts_va_arg_pointer_source` | `test/Verifier/tbaa-allowed.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_aggregate_vector.rs::extract_value_struct_field0` | `test/Bitcode/compatibility.ll` line 1549 | mirror |
| `crates/llvmkit-ir/tests/builder_aggregate_vector.rs::extract_value_array_index` | `test/Bitcode/compatibility.ll` line 1553 | mirror |
| `crates/llvmkit-ir/tests/builder_aggregate_vector.rs::extract_value_nested_indices` | `test/Bitcode/compatibility.ll` line 1555 | mirror |
| `crates/llvmkit-ir/tests/builder_aggregate_vector.rs::insert_value_struct_field0` | `test/Bitcode/compatibility.ll` line 1558 | mirror |
| `crates/llvmkit-ir/tests/builder_aggregate_vector.rs::insert_value_array_index_zero` | `test/Bitcode/compatibility.ll` line 1562 | mirror |
| `crates/llvmkit-ir/tests/builder_aggregate_vector.rs::extract_element_vector_i8_index` | `test/Bitcode/compatibility.ll` line 1535 | mirror |
| `crates/llvmkit-ir/tests/builder_aggregate_vector.rs::insert_element_vector_float_at_i8` | `test/Bitcode/compatibility.ll` line 1537 | mirror |
| `crates/llvmkit-ir/tests/builder_aggregate_vector.rs::shuffle_vector_zeroinitializer_mask` | `test/Bitcode/compatibility.ll` line 1539 | mirror |
| `crates/llvmkit-ir/tests/builder_aggregate_vector.rs::shuffle_vector_explicit_mask_print` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, ShuffleMaskQueries)` | mirror |
| `crates/llvmkit-ir/tests/builder_atomic.rs::fence_system_scope_orderings` | `test/Bitcode/compatibility.ll` lines 893-898 | mirror |
| `crates/llvmkit-ir/tests/builder_atomic.rs::fence_singlethread_seq_cst` | `test/Bitcode/compatibility.ll` line 899 | mirror |
| `crates/llvmkit-ir/tests/builder_atomic.rs::cmpxchg_no_align_monotonic_monotonic` | `test/Bitcode/compatibility.ll` line 810 | mirror |
| `crates/llvmkit-ir/tests/builder_atomic.rs::cmpxchg_weak_volatile_singlethread` | `test/Bitcode/compatibility.ll` line 824 | mirror |
| `crates/llvmkit-ir/tests/builder_atomic.rs::atomicrmw_xchg_monotonic` | `test/Bitcode/compatibility.ll` line 846 | mirror |
| `crates/llvmkit-ir/tests/builder_atomic.rs::atomicrmw_volatile_min_monotonic` | `test/Bitcode/compatibility.ll` line 862 | mirror |
| `crates/llvmkit-ir/tests/builder_atomic.rs::atomicrmw_umax_singlethread` | `test/Bitcode/compatibility.ll` line 864 | mirror |
| `crates/llvmkit-ir/tests/builder_var_arity_terminators.rs::switch_three_cases_print_form` | `test/Bitcode/compatibility.ll` lines 1302-1310 | mirror |
| `crates/llvmkit-ir/tests/builder_var_arity_terminators.rs::switch_no_cases_only_default` | `test/Assembler/2003-05-15-SwitchBug.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_var_arity_terminators.rs::indirectbr_single_destination` | `test/Bitcode/compatibility.ll` line 1320 | mirror |
| `crates/llvmkit-ir/tests/builder_var_arity_terminators.rs::indirectbr_multiple_destinations` | `test/Bitcode/compatibility.ll` line 1322 | mirror |
| `crates/llvmkit-ir/tests/builder_eh_calls.rs::invoke_void_to_unwind` | `test/Bitcode/compatibility.ll` line 1325 | mirror |
| `crates/llvmkit-ir/tests/builder_eh_calls.rs::callbr_void_with_one_indirect_dest` | `test/Assembler/callbr.ll` lines 8-9 | mirror |
| `crates/llvmkit-ir/tests/builder_eh_calls.rs::callbr_two_indirect_dests_print_form` | `test/Assembler/inline-asm-constraint-error.ll` | mirror |
| `crates/llvmkit-ir/tests/builder_eh_data.rs::landingpad_cleanup_only` | `test/Bitcode/compatibility.ll` line 789 | mirror |
| `crates/llvmkit-ir/tests/builder_eh_data.rs::landingpad_cleanup_plus_catch` | `test/Bitcode/compatibility.ll` lines 1782-1786 | mirror |
| `crates/llvmkit-ir/tests/builder_eh_data.rs::resume_i32_undef` | `test/Bitcode/compatibility.ll` line 1332 | mirror |
| `crates/llvmkit-ir/tests/builder_eh_data.rs::landingpad_followed_by_resume` | `test/Bitcode/compatibility.ll` lines 1330-1332 | mirror |
| `crates/llvmkit-ir/tests/builder_funclet.rs::catchswitch_within_none_unwind_to_caller` | `test/Bitcode/compatibility.ll` line 1351 | mirror |
| `crates/llvmkit-ir/tests/builder_funclet.rs::catchpad_within_catchswitch_empty_args` | `test/Bitcode/compatibility.ll` line 1354 | mirror |
| `crates/llvmkit-ir/tests/builder_funclet.rs::cleanuppad_within_none_empty_args` | `test/Bitcode/compatibility.ll` line 1378 | mirror |
| `crates/llvmkit-ir/tests/builder_funclet.rs::cleanupret_unwind_to_caller` | `test/Bitcode/compatibility.ll` line 1397 | mirror |
| `crates/llvmkit-ir/tests/builder_funclet.rs::catchret_to_label` | `test/Bitcode/compatibility.ll` line 1412 | mirror |
| `crates/llvmkit-ir/tests/concurrent_counter_example.rs::concurrent_counter_example_emits_locked_ir` | `https://llvm.org/docs/Atomics.html` (fence-based decomposition) + `test/Bitcode/compatibility.ll` lines 848 / 893-895 / 1302-1310 | example |
| `crates/llvmkit-ir/tests/builder_atomic_load_store.rs::load_atomic_monotonic_align4` | `test/Bitcode/compatibility.ll` line 902 | mirror |
| `crates/llvmkit-ir/tests/builder_atomic_load_store.rs::load_atomic_volatile_acquire_align8` | `test/Bitcode/compatibility.ll` line 904 | mirror |
| `crates/llvmkit-ir/tests/builder_atomic_load_store.rs::load_atomic_volatile_singlethread_seq_cst_align16` | `test/Bitcode/compatibility.ll` line 906 | mirror |
| `crates/llvmkit-ir/tests/builder_atomic_load_store.rs::store_atomic_monotonic_align4` | `test/Bitcode/compatibility.ll` line 909 | mirror |
| `crates/llvmkit-ir/tests/builder_atomic_load_store.rs::store_atomic_volatile_monotonic_align4` | `test/Bitcode/compatibility.ll` line 911 | mirror |
| `crates/llvmkit-ir/tests/builder_atomic_load_store.rs::store_atomic_volatile_singlethread_monotonic` | `test/Bitcode/compatibility.ll` line 913 | mirror |
| `crates/llvmkit-ir/tests/builder_atomic_load_store.rs::verifier_rejects_atomic_load_release_ordering` | `lib/IR/Verifier.cpp::Verifier::visitLoadInst` ("Load cannot have Release ordering") | mirror |
| `crates/llvmkit-ir/tests/builder_atomic_load_store.rs::verifier_rejects_atomic_store_acquire_ordering` | `lib/IR/Verifier.cpp::Verifier::visitStoreInst` ("Store cannot have Acquire ordering") | mirror |
| `crates/llvmkit-ir/tests/builder_atomic_load_store.rs::verifier_rejects_atomic_load_non_power_of_two_size` | `lib/IR/Verifier.cpp::Verifier::checkAtomicMemAccessSize` | mirror |
| `crates/llvmkit-ir/tests/builder_atomic_load_store.rs::bitcast_int_to_fp_emits_text` | `unittests/IR/PatternMatch.cpp::TEST_F(PatternMatchTest, BitCast)` (line 638; inverse `int -> fp` direction) | llvmkit-specific |
| `crates/llvmkit-ir/tests/builder_atomic_load_store.rs::bitcast_fp_to_int_emits_text` | `unittests/IR/PatternMatch.cpp::TEST_F(PatternMatchTest, BitCast)` (line 638-643) | mirror |
| `crates/llvmkit-ir/tests/builder_positioning_and_unary.rs::position_before_inserts_between_prev_and_anchor` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, DebugLoc)` (lines 1155-1190) | mirror |
| `crates/llvmkit-ir/tests/builder_positioning_and_unary.rs::position_past_allocas_anchors_after_alloca_prefix` | `IRBuilder.h::IRBuilder::SetInsertPointPastAllocas` (no upstream `TEST_F`; live use in `lib/Frontend/OpenMP/OMPIRBuilder.cpp`) | llvmkit-specific |
| `crates/llvmkit-ir/tests/builder_positioning_and_unary.rs::save_and_restore_insert_point_round_trip` | `unittests/Frontend/OpenMPIRBuilderTest.cpp` lines 244 / 253 (`Builder.saveIP()` / `Builder.restoreIP`) | mirror |
| `crates/llvmkit-ir/tests/builder_positioning_and_unary.rs::build_int_neg_emits_sub_zero` | `IRBuilder.h::IRBuilder::CreateNeg` + `test/Assembler/auto_upgrade_nvvm_intrinsics.ll` line 128 (`; CHECK-DAG: ... = sub i32 0, %a`) | llvmkit-specific |
| `crates/llvmkit-ir/tests/builder_positioning_and_unary.rs::build_int_neg_nsw_emits_sub_nsw` | `IRBuilder.h::IRBuilder::CreateNSWNeg` + closest `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, WrapFlags)` (line 773) | llvmkit-specific |
| `crates/llvmkit-ir/tests/builder_positioning_and_unary.rs::build_int_not_emits_xor_minus_one` | `IRBuilder.h::IRBuilder::CreateNot` (no upstream `TEST_F`) | llvmkit-specific |
| `crates/llvmkit-ir/tests/builder_positioning_and_unary.rs::build_pointer_cast_same_addrspace_emits_bitcast` | `IRBuilder.h::IRBuilder::CreatePointerBitCastOrAddrSpaceCast` + live use in `unittests/Frontend/OpenMPIRBuilderTest.cpp` line 6473 | llvmkit-specific |
| `crates/llvmkit-ir/tests/builder_positioning_and_unary.rs::build_is_null_emits_icmp_eq_null` | `IRBuilder.h::IRBuilder::CreateIsNull` (no dedicated `TEST_F`; sibling `CreateIsNotNull` used in `unittests/Frontend/OpenMPIRBuilderTest.cpp` line 1153) | llvmkit-specific |
| `crates/llvmkit-ir/tests/builder_positioning_and_unary.rs::build_is_not_null_emits_icmp_ne_null` | `unittests/Frontend/OpenMPIRBuilderTest.cpp` line 1153 (`Builder.CreateIsNotNull(F->arg_begin())`) | mirror |
| `crates/llvmkit-ir/tests/builder_fmf_and_phi.rs::fmf_propagates_from_builder_to_fadd` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)` (line 557) | mirror |
| `crates/llvmkit-ir/tests/builder_fmf_and_phi.rs::clear_fast_math_flags_drops_flags_from_subsequent_ops` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)` (line 622-628) | mirror |
| `crates/llvmkit-ir/tests/builder_fmf_and_phi.rs::fmf_propagates_to_fcmp_oeq` | `unittests/IR/IRBuilderTest.cpp::TEST_F(IRBuilderTest, FastMathFlags)` (lines 643-658, AllowReciprocal arm) | mirror |
| `crates/llvmkit-ir/tests/builder_fmf_and_phi.rs::build_fcmp_oeq_emits_oeq` | `test/Bitcode/compatibility.ll` line 1677 | mirror |
| `crates/llvmkit-ir/tests/builder_fmf_and_phi.rs::build_fcmp_ogt_emits_ogt` | `test/Bitcode/compatibility.ll` line 1679 | mirror |
| `crates/llvmkit-ir/tests/builder_fmf_and_phi.rs::build_fcmp_oge_emits_oge` | `test/Bitcode/compatibility.ll` line 1681 | mirror |
| `crates/llvmkit-ir/tests/builder_fmf_and_phi.rs::build_fcmp_olt_emits_olt` | `test/Bitcode/compatibility.ll` line 1683 | mirror |
| `crates/llvmkit-ir/tests/builder_fmf_and_phi.rs::build_fcmp_ole_emits_ole` | `test/Bitcode/compatibility.ll` line 1685 | mirror |
| `crates/llvmkit-ir/tests/builder_fmf_and_phi.rs::build_fcmp_ord_emits_ord` | `test/Bitcode/compatibility.ll` line 1689 | mirror |
| `crates/llvmkit-ir/tests/builder_fmf_and_phi.rs::build_fcmp_uno_emits_uno` | `test/Bitcode/compatibility.ll` line 1703 | mirror |
| `crates/llvmkit-ir/tests/builder_fmf_and_phi.rs::build_fcmp_ueq_emits_ueq` | `test/Bitcode/compatibility.ll` line 1691 | mirror |
| `crates/llvmkit-ir/tests/builder_fmf_and_phi.rs::build_fp_phi_emits_phi_with_double_kind` | `unittests/IR/InstructionsTest.cpp::TEST(InstructionsTest, FPMathOperator)` line 539 (`Builder.CreatePHI(getDoubleTy(), 0)`) | mirror |
| `crates/llvmkit-ir/tests/builder_fmf_and_phi.rs::build_pointer_phi_emits_phi_with_ptr` | `test/Verifier/inalloca2.ll` line 35 (`%args = phi ptr [ %a, %if ], [ %b, %else ]`) | llvmkit-specific |