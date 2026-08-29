//! DWARF constant tables. Mirrors `llvm/BinaryFormat/Dwarf.h` and
//! `llvm/IR/DebugInfoFlags.def`.
//!
//! The `.ll` parser needs these to reject an invalid `DW_*` spelling the way
//! `LLParser::parseMDField` does, and the AsmWriter needs the reverse direction
//! because upstream stores the *encoding* and prints the *name*: a source
//! `tag: 15` round-trips as `tag: DW_TAG_pointer_type`.
//!
//! # Where the numbers come from
//!
//! Generated from the vendored
//! `crates/llvmkit-asmparser/tablegen/llvm-22.1.4/include/llvm/BinaryFormat/Dwarf.def`
//! and `.../IR/DebugInfoFlags.def`, which are tracked in-tree for exactly this
//! reason — `orig_cpp/` is gitignored, so a test may not read it. The vendored
//! copies let `dwarf_def_drift.rs` re-derive every row and fail if this file
//! and upstream disagree, which is the same guarantee `attribute_td_drift.rs`
//! gives the attribute keyword table.
//!
//! Two families are *not* pure `.def` transcriptions, and both follow
//! `lib/BinaryFormat/Dwarf.cpp`:
//!
//! - `OPERATIONS` is the `HANDLE_DW_OP` family plus the eight
//!   `DW_OP_LLVM_*` cases `dwarf::getOperationEncoding` lists by hand. The
//!   `HANDLE_DW_OP_LLVM_USEROP` family is deliberately absent: upstream uses it
//!   only for *printing* (`LlvmUserOperationEncodingString`), never for the
//!   name lookup the parser performs.
//! - `MACINFO_TYPES` is `dwarf::getMacinfo`'s hand-written switch; there is
//!   no `HANDLE_DW_MACINFO` family in `Dwarf.def`.
//!
//! # Sentinels
//!
//! Upstream signals "unknown" with a magic return (`DW_TAG_invalid`, `0`,
//! `FlagZero`). These return [`Option`] instead — the same logic, spelled the
//! way the rest of llvmkit spells it.

/// `DW_TAG_*` name/value pairs, in `Dwarf.def` order.
pub(crate) const TAGS: &[(&str, u32)] = &[
    ("DW_TAG_null", 0x0),
    ("DW_TAG_array_type", 0x1),
    ("DW_TAG_class_type", 0x2),
    ("DW_TAG_entry_point", 0x3),
    ("DW_TAG_enumeration_type", 0x4),
    ("DW_TAG_formal_parameter", 0x5),
    ("DW_TAG_imported_declaration", 0x8),
    ("DW_TAG_label", 0xa),
    ("DW_TAG_lexical_block", 0xb),
    ("DW_TAG_member", 0xd),
    ("DW_TAG_pointer_type", 0xf),
    ("DW_TAG_reference_type", 0x10),
    ("DW_TAG_compile_unit", 0x11),
    ("DW_TAG_string_type", 0x12),
    ("DW_TAG_structure_type", 0x13),
    ("DW_TAG_subroutine_type", 0x15),
    ("DW_TAG_typedef", 0x16),
    ("DW_TAG_union_type", 0x17),
    ("DW_TAG_unspecified_parameters", 0x18),
    ("DW_TAG_variant", 0x19),
    ("DW_TAG_common_block", 0x1a),
    ("DW_TAG_common_inclusion", 0x1b),
    ("DW_TAG_inheritance", 0x1c),
    ("DW_TAG_inlined_subroutine", 0x1d),
    ("DW_TAG_module", 0x1e),
    ("DW_TAG_ptr_to_member_type", 0x1f),
    ("DW_TAG_set_type", 0x20),
    ("DW_TAG_subrange_type", 0x21),
    ("DW_TAG_with_stmt", 0x22),
    ("DW_TAG_access_declaration", 0x23),
    ("DW_TAG_base_type", 0x24),
    ("DW_TAG_catch_block", 0x25),
    ("DW_TAG_const_type", 0x26),
    ("DW_TAG_constant", 0x27),
    ("DW_TAG_enumerator", 0x28),
    ("DW_TAG_file_type", 0x29),
    ("DW_TAG_friend", 0x2a),
    ("DW_TAG_namelist", 0x2b),
    ("DW_TAG_namelist_item", 0x2c),
    ("DW_TAG_packed_type", 0x2d),
    ("DW_TAG_subprogram", 0x2e),
    ("DW_TAG_template_type_parameter", 0x2f),
    ("DW_TAG_template_value_parameter", 0x30),
    ("DW_TAG_thrown_type", 0x31),
    ("DW_TAG_try_block", 0x32),
    ("DW_TAG_variant_part", 0x33),
    ("DW_TAG_variable", 0x34),
    ("DW_TAG_volatile_type", 0x35),
    ("DW_TAG_dwarf_procedure", 0x36),
    ("DW_TAG_restrict_type", 0x37),
    ("DW_TAG_interface_type", 0x38),
    ("DW_TAG_namespace", 0x39),
    ("DW_TAG_imported_module", 0x3a),
    ("DW_TAG_unspecified_type", 0x3b),
    ("DW_TAG_partial_unit", 0x3c),
    ("DW_TAG_imported_unit", 0x3d),
    ("DW_TAG_condition", 0x3f),
    ("DW_TAG_shared_type", 0x40),
    ("DW_TAG_type_unit", 0x41),
    ("DW_TAG_rvalue_reference_type", 0x42),
    ("DW_TAG_template_alias", 0x43),
    ("DW_TAG_coarray_type", 0x44),
    ("DW_TAG_generic_subrange", 0x45),
    ("DW_TAG_dynamic_type", 0x46),
    ("DW_TAG_atomic_type", 0x47),
    ("DW_TAG_call_site", 0x48),
    ("DW_TAG_call_site_parameter", 0x49),
    ("DW_TAG_skeleton_unit", 0x4a),
    ("DW_TAG_immutable_type", 0x4b),
    ("DW_TAG_MIPS_loop", 0x4081),
    ("DW_TAG_format_label", 0x4101),
    ("DW_TAG_function_template", 0x4102),
    ("DW_TAG_class_template", 0x4103),
    ("DW_TAG_GNU_BINCL", 0x4104),
    ("DW_TAG_GNU_EINCL", 0x4105),
    ("DW_TAG_GNU_template_template_param", 0x4106),
    ("DW_TAG_GNU_template_parameter_pack", 0x4107),
    ("DW_TAG_GNU_formal_parameter_pack", 0x4108),
    ("DW_TAG_GNU_call_site", 0x4109),
    ("DW_TAG_GNU_call_site_parameter", 0x410a),
    ("DW_TAG_APPLE_property", 0x4200),
    ("DW_TAG_SUN_function_template", 0x4201),
    ("DW_TAG_SUN_class_template", 0x4202),
    ("DW_TAG_SUN_struct_template", 0x4203),
    ("DW_TAG_SUN_union_template", 0x4204),
    ("DW_TAG_SUN_indirect_inheritance", 0x4205),
    ("DW_TAG_SUN_codeflags", 0x4206),
    ("DW_TAG_SUN_memop_info", 0x4207),
    ("DW_TAG_SUN_omp_child_func", 0x4208),
    ("DW_TAG_SUN_rtti_descriptor", 0x4209),
    ("DW_TAG_SUN_dtor_info", 0x420a),
    ("DW_TAG_SUN_dtor", 0x420b),
    ("DW_TAG_SUN_f90_interface", 0x420c),
    ("DW_TAG_SUN_fortran_vax_structure", 0x420d),
    ("DW_TAG_SUN_hi", 0x42ff),
    ("DW_TAG_LLVM_ptrauth_type", 0x4300),
    ("DW_TAG_ALTIUM_circ_type", 0x5101),
    ("DW_TAG_ALTIUM_mwa_circ_type", 0x5102),
    ("DW_TAG_ALTIUM_rev_carry_type", 0x5103),
    ("DW_TAG_ALTIUM_rom", 0x5111),
    ("DW_TAG_LLVM_annotation", 0x6000),
    ("DW_TAG_GHS_namespace", 0x8004),
    ("DW_TAG_GHS_using_namespace", 0x8005),
    ("DW_TAG_GHS_using_declaration", 0x8006),
    ("DW_TAG_GHS_template_templ_param", 0x8007),
    ("DW_TAG_UPC_shared_type", 0x8765),
    ("DW_TAG_UPC_strict_type", 0x8766),
    ("DW_TAG_UPC_relaxed", 0x8767),
    ("DW_TAG_PGI_kanji_type", 0xa000),
    ("DW_TAG_PGI_interface_block", 0xa020),
    ("DW_TAG_BORLAND_property", 0xb000),
    ("DW_TAG_BORLAND_Delphi_string", 0xb001),
    ("DW_TAG_BORLAND_Delphi_dynamic_array", 0xb002),
    ("DW_TAG_BORLAND_Delphi_set", 0xb003),
    ("DW_TAG_BORLAND_Delphi_variant", 0xb004),
];

/// `DW_ATE_*` name/value pairs, in `Dwarf.def` order.
pub(crate) const ATTRIBUTE_ENCODINGS: &[(&str, u32)] = &[
    ("DW_ATE_address", 0x1),
    ("DW_ATE_boolean", 0x2),
    ("DW_ATE_complex_float", 0x3),
    ("DW_ATE_float", 0x4),
    ("DW_ATE_signed", 0x5),
    ("DW_ATE_signed_char", 0x6),
    ("DW_ATE_unsigned", 0x7),
    ("DW_ATE_unsigned_char", 0x8),
    ("DW_ATE_imaginary_float", 0x9),
    ("DW_ATE_packed_decimal", 0xa),
    ("DW_ATE_numeric_string", 0xb),
    ("DW_ATE_edited", 0xc),
    ("DW_ATE_signed_fixed", 0xd),
    ("DW_ATE_unsigned_fixed", 0xe),
    ("DW_ATE_decimal_float", 0xf),
    ("DW_ATE_UTF", 0x10),
    ("DW_ATE_UCS", 0x11),
    ("DW_ATE_ASCII", 0x12),
    ("DW_ATE_HP_complex_float", 0x81),
    ("DW_ATE_HP_float128", 0x82),
    ("DW_ATE_HP_complex_float128", 0x83),
    ("DW_ATE_HP_floathpintel", 0x84),
    ("DW_ATE_HP_imaginary_float90", 0x85),
    ("DW_ATE_HP_imaginary_float128", 0x86),
];

/// `DW_LANG_*` name/value pairs, in `Dwarf.def` order.
pub(crate) const LANGUAGES: &[(&str, u32)] = &[
    ("DW_LANG_C89", 0x1),
    ("DW_LANG_C", 0x2),
    ("DW_LANG_Ada83", 0x3),
    ("DW_LANG_C_plus_plus", 0x4),
    ("DW_LANG_Cobol74", 0x5),
    ("DW_LANG_Cobol85", 0x6),
    ("DW_LANG_Fortran77", 0x7),
    ("DW_LANG_Fortran90", 0x8),
    ("DW_LANG_Pascal83", 0x9),
    ("DW_LANG_Modula2", 0xa),
    ("DW_LANG_Java", 0xb),
    ("DW_LANG_C99", 0xc),
    ("DW_LANG_Ada95", 0xd),
    ("DW_LANG_Fortran95", 0xe),
    ("DW_LANG_PLI", 0xf),
    ("DW_LANG_ObjC", 0x10),
    ("DW_LANG_ObjC_plus_plus", 0x11),
    ("DW_LANG_UPC", 0x12),
    ("DW_LANG_D", 0x13),
    ("DW_LANG_Python", 0x14),
    ("DW_LANG_OpenCL", 0x15),
    ("DW_LANG_Go", 0x16),
    ("DW_LANG_Modula3", 0x17),
    ("DW_LANG_Haskell", 0x18),
    ("DW_LANG_C_plus_plus_03", 0x19),
    ("DW_LANG_C_plus_plus_11", 0x1a),
    ("DW_LANG_OCaml", 0x1b),
    ("DW_LANG_Rust", 0x1c),
    ("DW_LANG_C11", 0x1d),
    ("DW_LANG_Swift", 0x1e),
    ("DW_LANG_Julia", 0x1f),
    ("DW_LANG_Dylan", 0x20),
    ("DW_LANG_C_plus_plus_14", 0x21),
    ("DW_LANG_Fortran03", 0x22),
    ("DW_LANG_Fortran08", 0x23),
    ("DW_LANG_RenderScript", 0x24),
    ("DW_LANG_BLISS", 0x25),
    ("DW_LANG_Kotlin", 0x26),
    ("DW_LANG_Zig", 0x27),
    ("DW_LANG_Crystal", 0x28),
    ("DW_LANG_C_plus_plus_17", 0x2a),
    ("DW_LANG_C_plus_plus_20", 0x2b),
    ("DW_LANG_C17", 0x2c),
    ("DW_LANG_Fortran18", 0x2d),
    ("DW_LANG_Ada2005", 0x2e),
    ("DW_LANG_Ada2012", 0x2f),
    ("DW_LANG_HIP", 0x30),
    ("DW_LANG_Assembly", 0x31),
    ("DW_LANG_C_sharp", 0x32),
    ("DW_LANG_Mojo", 0x33),
    ("DW_LANG_GLSL", 0x34),
    ("DW_LANG_GLSL_ES", 0x35),
    ("DW_LANG_HLSL", 0x36),
    ("DW_LANG_OpenCL_CPP", 0x37),
    ("DW_LANG_CPP_for_OpenCL", 0x38),
    ("DW_LANG_SYCL", 0x39),
    ("DW_LANG_Metal", 0x3d),
    ("DW_LANG_Ruby", 0x40),
    ("DW_LANG_Move", 0x41),
    ("DW_LANG_Hylo", 0x42),
    ("DW_LANG_Mips_Assembler", 0x8001),
    ("DW_LANG_GOOGLE_RenderScript", 0x8e57),
    ("DW_LANG_BORLAND_Delphi", 0xb000),
];

/// `DW_LNAME_*` name/value pairs, in `Dwarf.def` order.
pub(crate) const SOURCE_LANGUAGE_NAMES: &[(&str, u32)] = &[
    ("DW_LNAME_Ada", 0x1),
    ("DW_LNAME_BLISS", 0x2),
    ("DW_LNAME_C", 0x3),
    ("DW_LNAME_C_plus_plus", 0x4),
    ("DW_LNAME_Cobol", 0x5),
    ("DW_LNAME_Crystal", 0x6),
    ("DW_LNAME_D", 0x7),
    ("DW_LNAME_Dylan", 0x8),
    ("DW_LNAME_Fortran", 0x9),
    ("DW_LNAME_Go", 0xa),
    ("DW_LNAME_Haskell", 0xb),
    ("DW_LNAME_Java", 0xc),
    ("DW_LNAME_Julia", 0xd),
    ("DW_LNAME_Kotlin", 0xe),
    ("DW_LNAME_Modula2", 0xf),
    ("DW_LNAME_Modula3", 0x10),
    ("DW_LNAME_ObjC", 0x11),
    ("DW_LNAME_ObjC_plus_plus", 0x12),
    ("DW_LNAME_OCaml", 0x13),
    ("DW_LNAME_OpenCL_C", 0x14),
    ("DW_LNAME_Pascal", 0x15),
    ("DW_LNAME_PLI", 0x16),
    ("DW_LNAME_Python", 0x17),
    ("DW_LNAME_RenderScript", 0x18),
    ("DW_LNAME_Rust", 0x19),
    ("DW_LNAME_Swift", 0x1a),
    ("DW_LNAME_UPC", 0x1b),
    ("DW_LNAME_Zig", 0x1c),
    ("DW_LNAME_Assembly", 0x1d),
    ("DW_LNAME_C_sharp", 0x1e),
    ("DW_LNAME_Mojo", 0x1f),
    ("DW_LNAME_GLSL", 0x20),
    ("DW_LNAME_GLSL_ES", 0x21),
    ("DW_LNAME_HLSL", 0x22),
    ("DW_LNAME_OpenCL_CPP", 0x23),
    ("DW_LNAME_CPP_for_OpenCL", 0x24),
    ("DW_LNAME_SYCL", 0x25),
    ("DW_LNAME_Ruby", 0x26),
    ("DW_LNAME_Move", 0x27),
    ("DW_LNAME_Hylo", 0x28),
    ("DW_LNAME_Metal", 0x2c),
];

/// `DW_CC_*` name/value pairs, in `Dwarf.def` order.
pub(crate) const CALLING_CONVENTIONS: &[(&str, u32)] = &[
    ("DW_CC_normal", 0x1),
    ("DW_CC_program", 0x2),
    ("DW_CC_nocall", 0x3),
    ("DW_CC_pass_by_reference", 0x4),
    ("DW_CC_pass_by_value", 0x5),
    ("DW_CC_GNU_renesas_sh", 0x40),
    ("DW_CC_GNU_borland_fastcall_i386", 0x41),
    ("DW_CC_BORLAND_safecall", 0xb0),
    ("DW_CC_BORLAND_stdcall", 0xb1),
    ("DW_CC_BORLAND_pascal", 0xb2),
    ("DW_CC_BORLAND_msfastcall", 0xb3),
    ("DW_CC_BORLAND_msreturn", 0xb4),
    ("DW_CC_BORLAND_thiscall", 0xb5),
    ("DW_CC_BORLAND_fastcall", 0xb6),
    ("DW_CC_LLVM_vectorcall", 0xc0),
    ("DW_CC_LLVM_Win64", 0xc1),
    ("DW_CC_LLVM_X86_64SysV", 0xc2),
    ("DW_CC_LLVM_AAPCS", 0xc3),
    ("DW_CC_LLVM_AAPCS_VFP", 0xc4),
    ("DW_CC_LLVM_IntelOclBicc", 0xc5),
    ("DW_CC_LLVM_SpirFunction", 0xc6),
    ("DW_CC_LLVM_DeviceKernel", 0xc7),
    ("DW_CC_LLVM_Swift", 0xc8),
    ("DW_CC_LLVM_PreserveMost", 0xc9),
    ("DW_CC_LLVM_PreserveAll", 0xca),
    ("DW_CC_LLVM_X86RegCall", 0xcb),
    ("DW_CC_LLVM_M68kRTD", 0xcc),
    ("DW_CC_LLVM_PreserveNone", 0xcd),
    ("DW_CC_LLVM_RISCVVectorCall", 0xce),
    ("DW_CC_LLVM_SwiftTail", 0xcf),
    ("DW_CC_LLVM_RISCVVLSCall", 0xd0),
    ("DW_CC_GDB_IBM_OpenCL", 0xff),
];

/// `DW_VIRTUALITY_*` name/value pairs, in `Dwarf.def` order.
pub(crate) const VIRTUALITIES: &[(&str, u32)] = &[
    ("DW_VIRTUALITY_none", 0x0),
    ("DW_VIRTUALITY_virtual", 0x1),
    ("DW_VIRTUALITY_pure_virtual", 0x2),
];

/// `DW_OP_*` name/value pairs, in `Dwarf.def` order.
pub(crate) const OPERATIONS: &[(&str, u32)] = &[
    ("DW_OP_addr", 0x3),
    ("DW_OP_deref", 0x6),
    ("DW_OP_const1u", 0x8),
    ("DW_OP_const1s", 0x9),
    ("DW_OP_const2u", 0xa),
    ("DW_OP_const2s", 0xb),
    ("DW_OP_const4u", 0xc),
    ("DW_OP_const4s", 0xd),
    ("DW_OP_const8u", 0xe),
    ("DW_OP_const8s", 0xf),
    ("DW_OP_constu", 0x10),
    ("DW_OP_consts", 0x11),
    ("DW_OP_dup", 0x12),
    ("DW_OP_drop", 0x13),
    ("DW_OP_over", 0x14),
    ("DW_OP_pick", 0x15),
    ("DW_OP_swap", 0x16),
    ("DW_OP_rot", 0x17),
    ("DW_OP_xderef", 0x18),
    ("DW_OP_abs", 0x19),
    ("DW_OP_and", 0x1a),
    ("DW_OP_div", 0x1b),
    ("DW_OP_minus", 0x1c),
    ("DW_OP_mod", 0x1d),
    ("DW_OP_mul", 0x1e),
    ("DW_OP_neg", 0x1f),
    ("DW_OP_not", 0x20),
    ("DW_OP_or", 0x21),
    ("DW_OP_plus", 0x22),
    ("DW_OP_plus_uconst", 0x23),
    ("DW_OP_shl", 0x24),
    ("DW_OP_shr", 0x25),
    ("DW_OP_shra", 0x26),
    ("DW_OP_xor", 0x27),
    ("DW_OP_bra", 0x28),
    ("DW_OP_eq", 0x29),
    ("DW_OP_ge", 0x2a),
    ("DW_OP_gt", 0x2b),
    ("DW_OP_le", 0x2c),
    ("DW_OP_lt", 0x2d),
    ("DW_OP_ne", 0x2e),
    ("DW_OP_skip", 0x2f),
    ("DW_OP_lit0", 0x30),
    ("DW_OP_lit1", 0x31),
    ("DW_OP_lit2", 0x32),
    ("DW_OP_lit3", 0x33),
    ("DW_OP_lit4", 0x34),
    ("DW_OP_lit5", 0x35),
    ("DW_OP_lit6", 0x36),
    ("DW_OP_lit7", 0x37),
    ("DW_OP_lit8", 0x38),
    ("DW_OP_lit9", 0x39),
    ("DW_OP_lit10", 0x3a),
    ("DW_OP_lit11", 0x3b),
    ("DW_OP_lit12", 0x3c),
    ("DW_OP_lit13", 0x3d),
    ("DW_OP_lit14", 0x3e),
    ("DW_OP_lit15", 0x3f),
    ("DW_OP_lit16", 0x40),
    ("DW_OP_lit17", 0x41),
    ("DW_OP_lit18", 0x42),
    ("DW_OP_lit19", 0x43),
    ("DW_OP_lit20", 0x44),
    ("DW_OP_lit21", 0x45),
    ("DW_OP_lit22", 0x46),
    ("DW_OP_lit23", 0x47),
    ("DW_OP_lit24", 0x48),
    ("DW_OP_lit25", 0x49),
    ("DW_OP_lit26", 0x4a),
    ("DW_OP_lit27", 0x4b),
    ("DW_OP_lit28", 0x4c),
    ("DW_OP_lit29", 0x4d),
    ("DW_OP_lit30", 0x4e),
    ("DW_OP_lit31", 0x4f),
    ("DW_OP_reg0", 0x50),
    ("DW_OP_reg1", 0x51),
    ("DW_OP_reg2", 0x52),
    ("DW_OP_reg3", 0x53),
    ("DW_OP_reg4", 0x54),
    ("DW_OP_reg5", 0x55),
    ("DW_OP_reg6", 0x56),
    ("DW_OP_reg7", 0x57),
    ("DW_OP_reg8", 0x58),
    ("DW_OP_reg9", 0x59),
    ("DW_OP_reg10", 0x5a),
    ("DW_OP_reg11", 0x5b),
    ("DW_OP_reg12", 0x5c),
    ("DW_OP_reg13", 0x5d),
    ("DW_OP_reg14", 0x5e),
    ("DW_OP_reg15", 0x5f),
    ("DW_OP_reg16", 0x60),
    ("DW_OP_reg17", 0x61),
    ("DW_OP_reg18", 0x62),
    ("DW_OP_reg19", 0x63),
    ("DW_OP_reg20", 0x64),
    ("DW_OP_reg21", 0x65),
    ("DW_OP_reg22", 0x66),
    ("DW_OP_reg23", 0x67),
    ("DW_OP_reg24", 0x68),
    ("DW_OP_reg25", 0x69),
    ("DW_OP_reg26", 0x6a),
    ("DW_OP_reg27", 0x6b),
    ("DW_OP_reg28", 0x6c),
    ("DW_OP_reg29", 0x6d),
    ("DW_OP_reg30", 0x6e),
    ("DW_OP_reg31", 0x6f),
    ("DW_OP_breg0", 0x70),
    ("DW_OP_breg1", 0x71),
    ("DW_OP_breg2", 0x72),
    ("DW_OP_breg3", 0x73),
    ("DW_OP_breg4", 0x74),
    ("DW_OP_breg5", 0x75),
    ("DW_OP_breg6", 0x76),
    ("DW_OP_breg7", 0x77),
    ("DW_OP_breg8", 0x78),
    ("DW_OP_breg9", 0x79),
    ("DW_OP_breg10", 0x7a),
    ("DW_OP_breg11", 0x7b),
    ("DW_OP_breg12", 0x7c),
    ("DW_OP_breg13", 0x7d),
    ("DW_OP_breg14", 0x7e),
    ("DW_OP_breg15", 0x7f),
    ("DW_OP_breg16", 0x80),
    ("DW_OP_breg17", 0x81),
    ("DW_OP_breg18", 0x82),
    ("DW_OP_breg19", 0x83),
    ("DW_OP_breg20", 0x84),
    ("DW_OP_breg21", 0x85),
    ("DW_OP_breg22", 0x86),
    ("DW_OP_breg23", 0x87),
    ("DW_OP_breg24", 0x88),
    ("DW_OP_breg25", 0x89),
    ("DW_OP_breg26", 0x8a),
    ("DW_OP_breg27", 0x8b),
    ("DW_OP_breg28", 0x8c),
    ("DW_OP_breg29", 0x8d),
    ("DW_OP_breg30", 0x8e),
    ("DW_OP_breg31", 0x8f),
    ("DW_OP_regx", 0x90),
    ("DW_OP_fbreg", 0x91),
    ("DW_OP_bregx", 0x92),
    ("DW_OP_piece", 0x93),
    ("DW_OP_deref_size", 0x94),
    ("DW_OP_xderef_size", 0x95),
    ("DW_OP_nop", 0x96),
    ("DW_OP_push_object_address", 0x97),
    ("DW_OP_call2", 0x98),
    ("DW_OP_call4", 0x99),
    ("DW_OP_call_ref", 0x9a),
    ("DW_OP_form_tls_address", 0x9b),
    ("DW_OP_call_frame_cfa", 0x9c),
    ("DW_OP_bit_piece", 0x9d),
    ("DW_OP_implicit_value", 0x9e),
    ("DW_OP_stack_value", 0x9f),
    ("DW_OP_implicit_pointer", 0xa0),
    ("DW_OP_addrx", 0xa1),
    ("DW_OP_constx", 0xa2),
    ("DW_OP_entry_value", 0xa3),
    ("DW_OP_const_type", 0xa4),
    ("DW_OP_regval_type", 0xa5),
    ("DW_OP_deref_type", 0xa6),
    ("DW_OP_xderef_type", 0xa7),
    ("DW_OP_convert", 0xa8),
    ("DW_OP_reinterpret", 0xa9),
    ("DW_OP_GNU_push_tls_address", 0xe0),
    ("DW_OP_HP_is_value", 0xe1),
    ("DW_OP_HP_fltconst4", 0xe2),
    ("DW_OP_HP_fltconst8", 0xe3),
    ("DW_OP_HP_mod_range", 0xe4),
    ("DW_OP_HP_unmod_range", 0xe5),
    ("DW_OP_HP_tls", 0xe6),
    ("DW_OP_INTEL_bit_piece", 0xe8),
    ("DW_OP_WASM_location", 0xed),
    ("DW_OP_WASM_location_int", 0xee),
    ("DW_OP_APPLE_uninit", 0xf0),
    ("DW_OP_GNU_implicit_pointer", 0xf2),
    ("DW_OP_GNU_entry_value", 0xf3),
    ("DW_OP_PGI_omp_thread_num", 0xf8),
    ("DW_OP_GNU_addr_index", 0xfb),
    ("DW_OP_GNU_const_index", 0xfc),
    ("DW_OP_LLVM_user", 0xe9),
    ("DW_OP_LLVM_fragment", 0x1000),
    ("DW_OP_LLVM_convert", 0x1001),
    ("DW_OP_LLVM_tag_offset", 0x1002),
    ("DW_OP_LLVM_entry_value", 0x1003),
    ("DW_OP_LLVM_implicit_pointer", 0x1004),
    ("DW_OP_LLVM_arg", 0x1005),
    ("DW_OP_LLVM_extract_bits_sext", 0x1006),
    ("DW_OP_LLVM_extract_bits_zext", 0x1007),
];

/// `DW_MACINFO_*` name/value pairs, in `Dwarf.def` order.
pub(crate) const MACINFO_TYPES: &[(&str, u32)] = &[
    ("DW_MACINFO_define", 0x1),
    ("DW_MACINFO_undef", 0x2),
    ("DW_MACINFO_start_file", 0x3),
    ("DW_MACINFO_end_file", 0x4),
    ("DW_MACINFO_vendor_ext", 0xff),
];

/// `DIFlag*` name/value pairs, in `DebugInfoFlags.def` order.
///
/// **`DIFlagLargest` is deliberately absent.** The `.def` guards that row
/// behind `#ifdef DI_FLAG_LARGEST_NEEDED`, which only `DebugInfoMetadata.h`
/// defines — to give the `DIFlags` enum a `LLVM_MARK_AS_BITMASK_ENUM` bound.
/// `DebugInfoMetadata.cpp` includes the same `.def` without it, so
/// `DINode::getFlag` never matches the spelling and `DINode::getFlagString`
/// has no `case` for its value (it could not: `FlagLargest` aliases
/// `FlagNameIsSimplified`). This table is the one behind those two routines,
/// so it stops where they stop. `DISPFlagLargest` is absent for the same
/// reason, and aliases `DISPFlagObjCDirect`.
pub(crate) const DI_FLAGS: &[(&str, u32)] = &[
    ("DIFlagZero", 0x0),
    ("DIFlagPrivate", 0x1),
    ("DIFlagProtected", 0x2),
    ("DIFlagPublic", 0x3),
    ("DIFlagFwdDecl", 0x4),
    ("DIFlagAppleBlock", 0x8),
    ("DIFlagReservedBit4", 0x10),
    ("DIFlagVirtual", 0x20),
    ("DIFlagArtificial", 0x40),
    ("DIFlagExplicit", 0x80),
    ("DIFlagPrototyped", 0x100),
    ("DIFlagObjcClassComplete", 0x200),
    ("DIFlagObjectPointer", 0x400),
    ("DIFlagVector", 0x800),
    ("DIFlagStaticMember", 0x1000),
    ("DIFlagLValueReference", 0x2000),
    ("DIFlagRValueReference", 0x4000),
    ("DIFlagExportSymbols", 0x8000),
    ("DIFlagSingleInheritance", 0x10000),
    ("DIFlagMultipleInheritance", 0x20000),
    ("DIFlagVirtualInheritance", 0x30000),
    ("DIFlagIntroducedVirtual", 0x40000),
    ("DIFlagBitField", 0x80000),
    ("DIFlagNoReturn", 0x100000),
    ("DIFlagTypePassByValue", 0x400000),
    ("DIFlagTypePassByReference", 0x800000),
    ("DIFlagEnumClass", 0x1000000),
    ("DIFlagThunk", 0x2000000),
    ("DIFlagNonTrivial", 0x4000000),
    ("DIFlagBigEndian", 0x8000000),
    ("DIFlagLittleEndian", 0x10000000),
    ("DIFlagAllCallsDescribed", 0x20000000),
    ("DIFlagNameIsSimplified", 0x40000000),
    ("DIFlagIndirectVirtualBase", 0x24),
];

/// `DISPFlag*` name/value pairs, in `DebugInfoFlags.def` order. See
/// [`DI_FLAGS`] for why `DISPFlagLargest` is absent.
pub(crate) const DISP_FLAGS: &[(&str, u32)] = &[
    ("DISPFlagZero", 0x0),
    ("DISPFlagVirtual", 0x1),
    ("DISPFlagPureVirtual", 0x2),
    ("DISPFlagLocalToUnit", 0x4),
    ("DISPFlagDefinition", 0x8),
    ("DISPFlagOptimized", 0x10),
    ("DISPFlagPure", 0x20),
    ("DISPFlagElemental", 0x40),
    ("DISPFlagRecursive", 0x80),
    ("DISPFlagMainSubprogram", 0x100),
    ("DISPFlagDeleted", 0x200),
    ("DISPFlagObjCDirect", 0x800),
];

/// `DW_APPLE_ENUM_KIND_*` name/value pairs, in `Dwarf.def` order.
///
/// The smallest family in the file — `HANDLE_DW_APPLE_ENUM_KIND` has two
/// entries — and the one `DICompositeType`'s `enumKind:` field is drawn from.
pub(crate) const APPLE_ENUM_KINDS: &[(&str, u32)] = &[
    ("DW_APPLE_ENUM_KIND_Closed", 0x00),
    ("DW_APPLE_ENUM_KIND_Open", 0x01),
];

/// Look `name` up in `table`.
fn lookup(table: &[(&str, u32)], name: &str) -> Option<u32> {
    table
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, value)| *value)
}

/// Look `value` up in `table`, returning the first name that carries it.
fn spelling(table: &'static [(&'static str, u32)], value: u32) -> Option<&'static str> {
    table
        .iter()
        .find(|(_, candidate)| *candidate == value)
        .map(|(name, _)| *name)
}

macro_rules! decl_lookup {
    ($name:ident, $string:ident, $table:ident, $what:literal) => {
        #[doc = concat!("The encoding for a ", $what, " spelling, or `None` if unknown.")]
        pub fn $name(spelling: &str) -> Option<u32> {
            lookup($table, spelling)
        }

        #[doc = concat!("The canonical ", $what, " spelling for an encoding, or `None`.")]
        pub fn $string(value: u32) -> Option<&'static str> {
            spelling($table, value)
        }
    };
}

decl_lookup!(tag, tag_string, TAGS, "`DW_TAG_*`");
decl_lookup!(
    attribute_encoding,
    attribute_encoding_string,
    ATTRIBUTE_ENCODINGS,
    "`DW_ATE_*`"
);
decl_lookup!(language, language_string, LANGUAGES, "`DW_LANG_*`");
decl_lookup!(
    source_language_name,
    source_language_name_string,
    SOURCE_LANGUAGE_NAMES,
    "`DW_LNAME_*`"
);
decl_lookup!(
    calling_convention,
    calling_convention_string,
    CALLING_CONVENTIONS,
    "`DW_CC_*`"
);
decl_lookup!(
    virtuality,
    virtuality_string,
    VIRTUALITIES,
    "`DW_VIRTUALITY_*`"
);
decl_lookup!(
    operation_encoding,
    operation_encoding_string,
    OPERATIONS,
    "`DW_OP_*`"
);
decl_lookup!(macinfo, macinfo_string, MACINFO_TYPES, "`DW_MACINFO_*`");
decl_lookup!(di_flag, di_flag_string, DI_FLAGS, "`DIFlag*`");
decl_lookup!(disp_flag, disp_flag_string, DISP_FLAGS, "`DISPFlag*`");
decl_lookup!(
    apple_enum_kind,
    apple_enum_kind_string,
    APPLE_ENUM_KINDS,
    "`DW_APPLE_ENUM_KIND_*`"
);
