//! Parity ledger for the KnownBits / ValueTracking surface.
//!
//! Anchors: `llvm/include/llvm/Support/KnownBits.h`,
//! `llvm/lib/Support/KnownBits.cpp`,
//! `llvm/include/llvm/Analysis/ValueTracking.h`,
//! `llvm/lib/Analysis/ValueTracking.cpp`,
//! `llvm/include/llvm/Analysis/DemandedBits.h`,
//! `llvm/lib/Analysis/DemandedBits.cpp`, and
//! `llvm/lib/Transforms/InstCombine/InstCombineSimplifyDemanded.cpp`.
//!
//! **No upstream counterpart.** A coverage ledger is an artifact of being a
//! reimplementation; LLVM has nothing to mirror it against.
//!
//! # What has teeth, and what does not
//!
//! `orig_cpp/` is gitignored, so — unlike `attribute_td_drift.rs`, which parses
//! a *tracked* vendored `Attributes.td` — this file cannot read the upstream
//! headers at test time. The guarantee is therefore split three ways:
//!
//! - **Enforced by the compiler.** `exercises_every_modeled_*` calls every
//!   entry in the modeled tables. Renaming or deleting one stops this file
//!   compiling, so the "modeled" column cannot silently become a lie.
//! - **Enforced at run time.** The tables are checked for the properties a
//!   ledger needs to stay readable: sorted, duplicate-free, and disjoint from
//!   the gap list, with a recorded reason on every gap. `every_modeled_*_row_is_exercised`
//!   additionally reads this file's own source and proves the matching
//!   exercise fn reaches *every* row — the two used to be linked by nothing
//!   but the fn's name, so a row could be added with no call to match it and
//!   still look enforced. That held for `KnownBits` from 2026-08-03 and for
//!   `ValueTracking` only from 2026-08-04, and in between the `ValueTracking`
//!   side had already drifted by four rows.
//! - **Maintained by hand.** The *gap* lists are a human record. Nothing here
//!   can notice that upstream grew a new method — that is the LLVM sync's job,
//!   which should re-derive the tables from the headers and reconcile. Both
//!   gap lists therefore name the release they were derived from.
//!
//! Saying that plainly is the point. This file used to assert that a `const`
//! array contained the strings it had just been initialised with; it could not
//! fail, and a ledger that looks authoritative while checking nothing is worse
//! than no ledger at all.

use std::collections::BTreeSet;

use llvmkit_ir::{ApInt, KnownBits};

/// The LLVM release the gap lists below were derived from. Bump it in the same
/// commit that reconciles them against a newer `KnownBits.h` /
/// `ValueTracking.h`.
const DERIVED_FROM_LLVM: &str = "22.1.4";

/// `KnownBits` operations llvmkit models, as `(upstream name, llvmkit name)`.
///
/// Sorted by upstream name. Where llvmkit splits one upstream overload set in
/// two (`udiv` / `udiv_with_exact`), both spellings are recorded together.
///
/// **The seven operators are listed too.** An earlier revision of this table
/// covered only the named member functions, because the grep that built it
/// looked for an identifier before a `(` and `operator<<=(` does not have one.
/// That is how `operator<<=` and `operator>>=` sat unmodeled while
/// `known_bits_public_surface_is_complete` reported the surface closed — see
/// that test's docs for what it can and cannot prove.
const MODELED_KNOWN_BITS: &[(&str, &str)] = &[
    ("abds", "abds"),
    ("abdu", "abdu"),
    ("abs", "abs / abs_with_int_min_poison"),
    ("add", "add / add_with_flags"),
    ("anyext", "anyext"),
    ("anyextOrTrunc", "anyext_or_trunc"),
    ("ashr", "ashr / ashr_with_flags"),
    ("avgCeilS", "avg_ceil_s"),
    ("avgCeilU", "avg_ceil_u"),
    ("avgFloorS", "avg_floor_s"),
    ("avgFloorU", "avg_floor_u"),
    ("blsi", "blsi"),
    ("blsmsk", "blsmsk"),
    ("byteSwap", "byte_swap"),
    ("computeForAddCarry", "compute_for_add_carry"),
    ("computeForAddSub", "compute_for_add_sub"),
    ("computeForSubBorrow", "compute_for_sub_borrow"),
    ("concat", "concat"),
    ("countMaxActiveBits", "count_max_active_bits"),
    ("countMaxLeadingOnes", "count_max_leading_ones"),
    ("countMaxLeadingZeros", "count_max_leading_zeros"),
    ("countMaxPopulation", "count_max_population"),
    ("countMaxSignificantBits", "count_max_significant_bits"),
    ("countMaxTrailingOnes", "count_max_trailing_ones"),
    ("countMaxTrailingZeros", "count_max_trailing_zeros"),
    ("countMinLeadingOnes", "count_min_leading_ones"),
    ("countMinLeadingZeros", "count_min_leading_zeros"),
    ("countMinPopulation", "count_min_population"),
    ("countMinSignBits", "count_min_sign_bits"),
    ("countMinTrailingOnes", "count_min_trailing_ones"),
    ("countMinTrailingZeros", "count_min_trailing_zeros"),
    // Debug printing. Upstream's `print` takes a raw_ostream; `dump` is the
    // `#if !defined(NDEBUG)` debugger convenience that calls it.
    ("dump", "derived Debug"),
    ("eq", "eq"),
    ("extractBits", "extract_bits"),
    ("getBitWidth", "bit_width"),
    ("getConstant", "constant"),
    ("getMaxValue", "max_value"),
    ("getMinValue", "min_value"),
    ("getSignedMaxValue", "signed_max_value"),
    ("getSignedMinValue", "signed_min_value"),
    ("hasConflict", "has_conflict"),
    ("haveNoCommonBitsSet", "have_no_common_bits_set"),
    ("insertBits", "insert_bits"),
    ("intersectWith", "intersect_with"),
    ("isAllOnes", "is_all_ones"),
    ("isConstant", "is_constant"),
    ("isNegative", "is_negative"),
    ("isNonNegative", "is_non_negative"),
    ("isNonPositive", "is_non_positive"),
    ("isNonZero", "is_non_zero"),
    ("isSignUnknown", "is_sign_unknown"),
    ("isStrictlyPositive", "is_strictly_positive"),
    ("isUnknown", "is_unknown"),
    ("isZero", "is_zero"),
    ("lshr", "lshr / lshr_with_flags"),
    ("makeConstant", "make_constant"),
    ("makeGE", "make_ge"),
    ("makeNegative", "make_negative"),
    ("makeNonNegative", "make_non_negative"),
    ("mul", "mul"),
    ("mulhs", "mulhs"),
    ("mulhu", "mulhu"),
    ("ne", "ne"),
    // The operators. Rust spells each as the std trait that means the same
    // thing, so these are trait impls rather than inherent methods.
    ("operator!=", "derived PartialEq"),
    ("operator&=", "BitAndAssign / BitAnd, over bitand"),
    ("operator<<=", "ShlAssign<u32> / Shl<u32>"),
    ("operator==", "derived PartialEq / Eq"),
    ("operator>>=", "ShrAssign<u32> / Shr<u32>"),
    ("operator^=", "BitXorAssign / BitXor, over bitxor"),
    ("operator|=", "BitOrAssign / BitOr, over bitor"),
    ("print", "Display"),
    ("reduceAdd", "reduce_add"),
    ("resetAll", "unknown"),
    ("reverseBits", "reverse_bits"),
    ("sadd_sat", "sadd_sat"),
    ("sdiv", "sdiv / sdiv_with_exact"),
    ("setAllConflict", "set_all_conflict"),
    ("setAllOnes", "set_all_ones"),
    ("setAllZero", "set_all_zero"),
    ("sext", "sext"),
    ("sextInReg", "sext_in_reg"),
    ("sextOrTrunc", "sext_or_trunc"),
    ("sge", "sge"),
    ("sgt", "sgt"),
    ("shl", "shl / shl_with_flags"),
    ("sle", "sle"),
    ("slt", "slt"),
    ("smax", "smax"),
    ("smin", "smin"),
    ("srem", "srem"),
    ("ssub_sat", "ssub_sat"),
    ("sub", "sub / sub_with_flags"),
    ("trunc", "trunc"),
    ("uadd_sat", "uadd_sat"),
    ("udiv", "udiv / udiv_with_exact"),
    ("uge", "uge"),
    ("ugt", "ugt"),
    ("ule", "ule"),
    ("ult", "ult"),
    ("umax", "umax"),
    ("umin", "umin"),
    ("unionWith", "union_with"),
    ("urem", "urem"),
    ("usub_sat", "usub_sat"),
    ("zext", "zext"),
    ("zextOrTrunc", "zext_or_trunc"),
];

/// `KnownBits` operations llvmkit does **not** model.
///
/// **Empty**, as of the audit dated in [`KNOWN_BITS_SURFACE_AUDITED`]. Every
/// operation `KnownBits.h` declares public is modeled; see
/// [`KNOWN_BITS_PRIVATE_UPSTREAM`] for the two that look missing but are
/// upstream internals.
///
/// Being empty is a *recorded finding*, not a proof — read
/// `known_bits_public_surface_is_complete` before trusting it.
const KNOWN_BITS_GAPS: &[(&str, &str)] = &[];

/// When the `KnownBits.h` public surface was last enumerated and diffed
/// against llvmkit, and how many public members it had.
///
/// The 2026-08-03 pass is the one that found `operator<<=` / `operator>>=`
/// missing. 106 = 99 named members + 7 operators.
///
/// # Reproducing the audit
///
/// Do not grep the raw header — that is what missed the operators, since
/// `operator<<=(` has no identifier before the paren, and it also trips over
/// `LLVM_DUMP_METHOD` expanding to attributes in front of `dump`. Preprocess
/// instead, so macros and `#if` are already resolved:
///
/// ```text
/// g++ -std=c++17 -fno-exceptions -fno-rtti -E -x c++ \
///   -I build/llvm/include \
///   -I orig_cpp/llvm-project-llvmorg-22.1.4/llvm/include \
///   -I orig_cpp/llvm-project-llvmorg-22.1.4/third-party/siphash/include \
///   orig_cpp/llvm-project-llvmorg-22.1.4/llvm/include/llvm/Support/KnownBits.h
/// ```
///
/// Then brace-match `struct KnownBits`, track the access specifier, strip
/// `__attribute__((...))`, and take the identifier (or `operator<op>`) before
/// each declaration's paren. That run reproduces both this count and
/// [`KNOWN_BITS_PRIVATE_UPSTREAM`] exactly.
///
/// `build/llvm/include` holds the CMake-generated `llvm-config.h` /
/// `abi-breaking.h`; without a configured build the header cannot be
/// preprocessed at all, which is the other reason this cannot run in CI.
const KNOWN_BITS_SURFACE_AUDITED: (&str, usize) = ("2026-08-03", 106);

/// How to reach the rows whose llvmkit column names a trait impl rather than a
/// method, as `(column text, syntax that exercises it)`.
///
/// `every_modeled_known_bits_row_is_exercised` looks for a `.name(` call for
/// an ordinary row; a trait impl has no such call site, so those rows say here
/// what to look for instead.
const KNOWN_BITS_TRAIT_PROBES: &[(&str, &[&str])] = &[
    ("derived PartialEq", &["==", "!="]),
    ("derived PartialEq / Eq", &["==", "!="]),
    ("derived Debug", &["{a:?}"]),
    ("Display", &["{a}"]),
    ("BitAndAssign / BitAnd, over bitand", &["&a & &b", "&= &b"]),
    ("BitOrAssign / BitOr, over bitor", &["&a | &b", "|= &b"]),
    ("BitXorAssign / BitXor, over bitxor", &["&a ^ &b", "^= &b"]),
    ("ShlAssign<u32> / Shl<u32>", &["<< 2", "<<= 1"]),
    ("ShrAssign<u32> / Shr<u32>", &[">> 2", ">>= 1"]),
];

/// `KnownBits` members that are **private** in `KnownBits.h` and so are not
/// part of the surface at all.
///
/// Both are declared outside the `public:` section upstream (`flipSignBit`
/// above the first `public:`, `remGetLowBits` under the trailing `private:`),
/// and both exist in llvmkit as module-private free functions in
/// `known_bits.rs`. An earlier revision of this ledger listed them as gaps;
/// that was wrong in both directions — they are neither public upstream nor
/// absent here.
const KNOWN_BITS_PRIVATE_UPSTREAM: &[(&str, &str)] = &[
    ("flipSignBit", "known_bits.rs::flip_sign_bit"),
    ("remGetLowBits", "known_bits.rs::rem_get_low_bits"),
];

/// ValueTracking entry points llvmkit models, as `(upstream, llvmkit)`.
///
/// llvmkit ports the known-bits core, the value-level predicates built directly
/// on it, the poison/undef reasoning, and the constant-range and overflow
/// families. The families still absent are listed in [`VALUE_TRACKING_GAPS`].
///
/// `is_known_one` and `is_known_zero` are exercised alongside these but are
/// absent from the table on purpose — they are llvmkit-specific conveniences
/// with no upstream entry point of their own. Upstream's `MaskedValueIsZero`,
/// which does take a mask, maps to `masked_value_is_zero`.
const MODELED_VALUE_TRACKING: &[(&str, &str)] = &[
    ("ComputeMaxSignificantBits", "compute_max_significant_bits"),
    ("ComputeNumSignBits", "compute_num_sign_bits"),
    ("ConstantDataArraySlice", "ConstantDataArraySlice"),
    ("FindInsertedValue", "find_inserted_value"),
    (
        "GetPointerBaseWithConstantOffset",
        "pointer_base_with_constant_offset",
    ),
    ("GetStringLength", "get_string_length"),
    ("MaskedValueIsZero", "masked_value_is_zero"),
    ("OverflowResult", "OverflowResult"),
    ("SelectPatternFlavor", "SelectPatternFlavor"),
    ("SelectPatternNaNBehavior", "SelectPatternNaNBehavior"),
    ("SelectPatternResult", "SelectPatternResult"),
    (
        "adjustKnownBitsForSelectArm",
        "adjust_known_bits_for_select_arm",
    ),
    (
        "adjustKnownFPClassForSelectArm",
        "adjust_known_fp_class_for_select_arm",
    ),
    (
        "analyzeKnownBitsFromAndXorOr",
        "analyze_known_bits_from_and_xor_or",
    ),
    (
        "canConvertToMinOrMaxIntrinsic",
        "can_convert_to_min_or_max_intrinsic",
    ),
    ("canCreatePoison", "can_create_poison"),
    ("canCreateUndefOrPoison", "can_create_undef_or_poison"),
    ("canIgnoreSignBitOfNaN", "can_ignore_sign_bit_of_nan"),
    ("canIgnoreSignBitOfZero", "can_ignore_sign_bit_of_zero"),
    ("cannotBeNegativeZero", "cannot_be_negative_zero"),
    (
        "cannotBeOrderedLessThanZero",
        "cannot_be_ordered_less_than_zero",
    ),
    ("collectPossibleValues", "collect_possible_values"),
    ("computeConstantRange", "compute_constant_range"),
    (
        "computeConstantRangeIncludingKnownBits",
        "compute_constant_range_including_known_bits",
    ),
    ("computeKnownBits", "compute_known_bits"),
    (
        "computeKnownBitsFromContext",
        "compute_known_bits_from_context",
    ),
    ("computeKnownFPClass", "compute_known_fp_class"),
    ("computeKnownFPSignBit", "compute_known_fp_sign_bit"),
    (
        "computeOverflowForSignedAdd",
        "compute_overflow_for_signed_add",
    ),
    (
        "computeOverflowForSignedMul",
        "compute_overflow_for_signed_mul",
    ),
    (
        "computeOverflowForSignedSub",
        "compute_overflow_for_signed_sub",
    ),
    (
        "computeOverflowForUnsignedAdd",
        "compute_overflow_for_unsigned_add",
    ),
    (
        "computeOverflowForUnsignedMul",
        "compute_overflow_for_unsigned_mul",
    ),
    (
        "computeOverflowForUnsignedSub",
        "compute_overflow_for_unsigned_sub",
    ),
    ("findAllocaForValue", "find_alloca_for_value"),
    (
        "findValuesAffectedByCondition",
        "find_values_affected_by_condition",
    ),
    (
        "getArgumentAliasingToReturnedPointer",
        "argument_aliasing_to_returned_pointer",
    ),
    ("getConstantDataArrayInfo", "get_constant_data_array_info"),
    ("getConstantStringInfo", "get_constant_string_info"),
    (
        "getInverseMinMaxFlavor",
        "SelectPatternFlavor::inverse_min_max",
    ),
    (
        "getInverseMinMaxIntrinsic",
        "MinMaxOperation::inverse / MinMaxIntrinsic::inverse / MinMaxKind::inverse",
    ),
    (
        "getMinMaxIntrinsic",
        "SelectPatternFlavor::min_max_intrinsic",
    ),
    ("getMinMaxLimit", "SelectPatternFlavor::min_max_limit"),
    ("getMinMaxPred", "SelectPatternFlavor::min_max_predicate"),
    ("getSelectPattern", "get_select_pattern"),
    ("getUnderlyingObject", "get_underlying_object"),
    (
        "getUnderlyingObjectAggressive",
        "get_underlying_object_aggressive",
    ),
    ("getUnderlyingObjects", "get_underlying_objects"),
    (
        "getUnderlyingObjectsForCodeGen",
        "get_underlying_objects_for_code_gen",
    ),
    ("haveNoCommonBitsSet", "have_no_common_bits_set"),
    ("impliesPoison", "implies_poison"),
    ("intrinsicPropagatesPoison", "intrinsic_propagates_poison"),
    ("isAssumeLikeIntrinsic", "is_assume_like_intrinsic"),
    ("isBytewiseValue", "is_bytewise_value"),
    ("isGuaranteedNotToBePoison", "is_known_not_poison"),
    ("isGuaranteedNotToBeUndef", "is_known_not_undef"),
    (
        "isGuaranteedNotToBeUndefOrPoison",
        "is_known_not_undef_or_poison",
    ),
    (
        "isGuaranteedToExecuteForEveryIteration",
        "is_guaranteed_to_execute_for_every_iteration",
    ),
    (
        "isGuaranteedToTransferExecutionToSuccessor",
        "is_guaranteed_to_transfer_execution_to_successor / block_transfers_execution_to_successor / instructions_transfer_execution_to_successor",
    ),
    ("isImpliedByDomCondition", "is_implied_by_dom_condition"),
    ("isImpliedCondition", "is_implied_condition"),
    (
        "isIntrinsicReturningPointerAliasingArgumentWithoutCapturing",
        "is_intrinsic_returning_pointer_aliasing_argument_without_capturing",
    ),
    ("isKnownInversion", "is_known_inversion"),
    ("isKnownNegation", "is_known_negation"),
    ("isKnownNegative", "is_known_negative"),
    ("isKnownNeverInfOrNaN", "is_known_never_infinity_or_nan"),
    ("isKnownNeverInfinity", "is_known_never_infinity"),
    ("isKnownNeverNaN", "is_known_never_nan"),
    ("isKnownNonEqual", "is_known_non_equal"),
    ("isKnownNonNegative", "is_known_non_negative"),
    ("isKnownNonZero", "is_known_non_zero"),
    ("isKnownPositive", "is_known_positive"),
    ("isKnownToBeAPowerOfTwo", "is_known_to_be_a_power_of_two"),
    ("isNotCrossLaneOperation", "is_not_cross_lane_operation"),
    (
        "isOnlyUsedInZeroComparison",
        "is_only_used_in_zero_comparison",
    ),
    (
        "isOnlyUsedInZeroEqualityComparison",
        "is_only_used_in_zero_equality_comparison",
    ),
    (
        "isSafeToSpeculativelyExecute",
        "is_safe_to_speculatively_execute",
    ),
    (
        "isSafeToSpeculativelyExecuteWithOpcode",
        "is_safe_to_speculatively_execute_with_opcode",
    ),
    (
        "isSafeToSpeculativelyExecuteWithVariableReplaced",
        "is_safe_to_speculatively_execute_with_variable_replaced",
    ),
    ("isSignBitCheck", "is_sign_bit_check"),
    ("isValidAssumeForContext", "is_valid_assume_for_context"),
    (
        "matchDecomposedSelectPattern",
        "match_decomposed_select_pattern",
    ),
    ("matchSelectPattern", "match_select_pattern"),
    (
        "mayHaveNonDefUseDependency",
        "may_have_non_def_use_dependency",
    ),
    (
        "mustExecuteUBIfPoisonOnPathTo",
        "must_execute_ub_if_poison_on_path_to",
    ),
    ("mustTriggerUB", "must_trigger_ub"),
    ("onlyUsedByLifetimeMarkers", "only_used_by_lifetime_markers"),
    (
        "onlyUsedByLifetimeMarkersOrDroppableInsts",
        "only_used_by_lifetime_markers_or_droppable_instructions",
    ),
    ("programUndefinedIfPoison", "program_undefined_if_poison"),
    (
        "programUndefinedIfUndefOrPoison",
        "program_undefined_if_undef_or_poison",
    ),
    ("propagatesPoison", "propagates_poison"),
    ("stripNullTest", "strip_null_test"),
    ("willNotFreeBetween", "will_not_free_between"),
];

/// The `ValueTracking.h` entry points llvmkit does not model, one row per
/// symbol, each naming the family and what blocks it.
///
/// Derived at LLVM [`DERIVED_FROM_LLVM`]. **This table used to be keyed by
/// family** — seven prose rows on the theory that "enumerating ~76
/// individually would be noise". The 2026-08-03 audit
/// ([`VALUE_TRACKING_SURFACE_AUDITED`]) showed what that cost: of 101 entry
/// points the header declares, 47 appeared in neither the modeled table nor
/// any gap reason. They were not recorded as missing; they were simply
/// invisible, and the ledger read as though the gap were seven families wide
/// rather than sixty-nine symbols.
///
/// Symbol-keyed costs more lines and buys the property that matters: modeled
/// plus gaps has to add up to the audited surface, which
/// `value_tracking_surface_is_accounted_for` asserts.
const VALUE_TRACKING_GAPS: &[(&str, &str)] = &[
    (
        "analyzeKnownFPClassFromSelect",
        "declared in ValueTracking.h and defined nowhere in the LLVM tree - the name occurs exactly once across llvm/, its own declaration, with no definition and no caller. There is no behaviour to port. The select arm it names is real and is ported, as adjustKnownFPClassForSelectArm plus the Select case of computeKnownFPClass",
    ),
    (
        "computeKnownBitsFromRangeMetadata",
        "modeled as the crate-private range_metadata_known_bits; public upstream, not public here",
    ),
    (
        "getFlippedStrictnessPredicateAndConstant",
        "residue: InstCombine predicate canonicalisation, which llvmkit does not have",
    ),
    (
        "getIntrinsicForCallSite",
        "blocked on there being no public intrinsic-id type: the TargetLibraryInfo half exists (target_library_info.rs::LibFunc, lib_func_for_name), but the return type is Intrinsic::ID over the whole intrinsic space and llvmkit's IntrinsicSemantic is pub(crate), which -D warnings makes unspellable in a public signature. Unlike getInverseMinMaxIntrinsic's ten-symbol family, this range is open-ended, so a hand-written sum type is not the answer",
    ),
    (
        "getVScaleRange",
        "blocked on the `vscale_range` attribute itself, which attribute_td_drift.rs lists as NOT_YET_MODELED: upstream reads a packed (min, max) pair and llvmkit's payload is a single u64, so porting it would mean inventing the second half",
    ),
    (
        "isOverflowIntrinsicNoWrap",
        "residue: reads the with-overflow intrinsics llvmkit models as plain calls",
    ),
    (
        "matchSimpleBinaryIntrinsicRecurrence",
        "the min/max/intrinsic sibling of matchSimpleRecurrence; needs the intrinsic recurrence forms llvmkit does not match yet",
    ),
    (
        "matchSimpleRecurrence",
        "implemented as the crate-private match_simple_recurrence, which the phi arm of computeKnownBits uses; public upstream, not public here",
    ),
];

/// When the `ValueTracking.h` surface was last enumerated and diffed against
/// llvmkit, and how many entry points it declared.
///
/// 101 = 96 namespace-scope functions + 5 types defined in the header
/// (`ConstantDataArraySlice`, `OverflowResult`, `SelectPatternFlavor`,
/// `SelectPatternNaNBehavior`, `SelectPatternResult`). Overloads collapse to
/// one row per name, which is the granularity a parity ledger needs.
///
/// # Reproducing the audit
///
/// Same method as [`KNOWN_BITS_SURFACE_AUDITED`], with one addition: the
/// declarations here sit at namespace scope rather than inside one struct, so
/// the preprocessed output has to be attributed back to its originating header
/// using the `# <line> "<file>"` markers before parsing. Keep only the regions
/// whose file is `Analysis/ValueTracking.h`, then split at namespace depth 1.
const VALUE_TRACKING_SURFACE_AUDITED: (&str, usize) = ("2026-08-03", 101);

/// `ValueTracking` entry points that are **internal to `ValueTracking.cpp`**
/// and so are not part of the header's surface.
///
/// `computeKnownBitsFromOperator` is `static` in the `.cpp`. llvmkit models it
/// anyway, as `known_bits_from_operator`, because the known-bits walk needs
/// it — but an earlier revision listed it in the *modeled* table, which claims
/// to track `ValueTracking.h`. It is the same category as `KnownBits.h`'s
/// `flipSignBit` / `remGetLowBits`, and gets the same treatment.
const VALUE_TRACKING_PRIVATE_UPSTREAM: &[(&str, &str)] = &[(
    "computeKnownBitsFromOperator",
    "static in ValueTracking.cpp; modeled here as known_bits_from_operator",
)];

/// Two representative 8-bit values to drive every modeled operation with.
fn operands() -> (KnownBits, KnownBits) {
    (
        KnownBits::from_ap_int(ApInt::from_words(8, &[0b1010_1010])),
        KnownBits::from_ap_int(ApInt::from_words(8, &[0b0000_1111])),
    )
}

/// Every entry in [`MODELED_KNOWN_BITS`] is a real, callable API.
///
/// A compile-time claim first: the ledger's "modeled" column cannot drift from
/// the crate, because a rename or deletion stops this file compiling. No
/// upstream counterpart — see the module docs.
#[test]
fn exercises_every_modeled_known_bits_operation() {
    let (a, b) = operands();
    let one_bit = KnownBits::from_ap_int(ApInt::from_words(1, &[0]));

    // Constructors and width changes.
    let _ = KnownBits::unknown(8);
    let _ = KnownBits::make_constant(ApInt::from_words(8, &[5]));
    let _ = a.trunc(4);
    let _ = a.anyext(16);
    let _ = a.zext(16);
    let _ = a.sext(16);
    let _ = a.anyext_or_trunc(4);
    let _ = a.zext_or_trunc(16);
    let _ = a.sext_or_trunc(16);
    let _ = a.extract_bits(4, 2);
    let _ = a.concat(&b);
    let _ = a.sext_in_reg(4);
    let _ = a.make_ge(&ApInt::from_words(8, &[3]));

    // Queries.
    let _ = a.bit_width();
    let _ = a.has_conflict();
    let _ = a.is_unknown();
    let _ = a.is_sign_unknown();
    let _ = a.is_constant();
    // `getConstant` upstream, which asserts `isConstant()`; llvmkit returns
    // `Option` instead of asserting.
    let _ = a.constant();
    let _ = a.is_known_zero(0);
    let _ = a.is_known_one(1);
    let _ = a.is_zero();
    let _ = a.is_all_ones();
    let _ = a.is_negative();
    let _ = a.is_non_negative();
    let _ = a.is_non_zero();
    let _ = a.is_strictly_positive();
    let _ = a.is_non_positive();
    let _ = a.min_value();
    let _ = a.max_value();
    let _ = a.signed_min_value();
    let _ = a.signed_max_value();

    // Bit counts.
    let _ = a.count_min_trailing_zeros();
    let _ = a.count_min_trailing_ones();
    let _ = a.count_min_leading_zeros();
    let _ = a.count_min_leading_ones();
    let _ = a.count_min_sign_bits();
    let _ = a.count_max_significant_bits();
    let _ = a.count_max_trailing_zeros();
    let _ = a.count_max_trailing_ones();
    let _ = a.count_max_leading_zeros();
    let _ = a.count_max_leading_ones();
    let _ = a.count_min_population();
    let _ = a.count_max_population();
    let _ = a.count_max_active_bits();

    // Lattice joins and in-place setters.
    let _ = a.intersect_with(&b);
    let _ = a.union_with(&b);
    let mut scratch = a.clone();
    scratch.set_all_zero();
    scratch.set_all_ones();
    scratch.set_all_conflict();
    scratch.insert_bits(&one_bit, 0);
    scratch.make_negative();
    scratch.make_non_negative();

    // Arithmetic and logic transfer functions.
    let _ = KnownBits::have_no_common_bits_set(&a, &b);
    let _ = KnownBits::bitand(&a, &b);
    let _ = KnownBits::bitor(&a, &b);
    let _ = KnownBits::bitxor(&a, &b);
    let _ = KnownBits::add(&a, &b);
    let _ = KnownBits::add_with_flags(&a, &b, false, false);
    let _ = KnownBits::compute_for_add_carry(&a, &b, &one_bit);
    let _ = KnownBits::compute_for_add_sub(true, false, false, &a, &b);
    let _ = KnownBits::compute_for_sub_borrow(&a, b.clone(), &one_bit);
    let _ = KnownBits::sub(&a, &b);
    let _ = KnownBits::sub_with_flags(&a, &b, false, false);
    let _ = KnownBits::mul(&a, &b);
    let _ = KnownBits::mulhs(&a, &b);
    let _ = KnownBits::mulhu(&a, &b);
    let _ = KnownBits::shl(&a, &b);
    let _ = KnownBits::shl_with_flags(&a, &b, false, false, false);
    let _ = KnownBits::lshr(&a, &b);
    let _ = KnownBits::lshr_with_flags(&a, &b, false, false);
    let _ = KnownBits::ashr(&a, &b);
    let _ = KnownBits::ashr_with_flags(&a, &b, false, false);
    let _ = KnownBits::udiv(&a, &b);
    let _ = KnownBits::udiv_with_exact(&a, &b, false);
    let _ = KnownBits::sdiv(&a, &b);
    let _ = KnownBits::sdiv_with_exact(&a, &b, false);
    let _ = KnownBits::urem(&a, &b);
    let _ = KnownBits::srem(&a, &b);

    // Saturating, averaging, min/max, absolute difference.
    let _ = KnownBits::sadd_sat(&a, &b);
    let _ = KnownBits::uadd_sat(&a, &b);
    let _ = KnownBits::ssub_sat(&a, &b);
    let _ = KnownBits::usub_sat(&a, &b);
    let _ = KnownBits::avg_floor_s(&a, &b);
    let _ = KnownBits::avg_floor_u(&a, &b);
    let _ = KnownBits::avg_ceil_s(&a, &b);
    let _ = KnownBits::avg_ceil_u(&a, &b);
    let _ = KnownBits::umin(&a, &b);
    let _ = KnownBits::umax(&a, &b);
    let _ = KnownBits::smin(&a, &b);
    let _ = KnownBits::smax(&a, &b);
    let _ = KnownBits::abdu(&a, &b);
    let _ = KnownBits::abds(&a, &b);

    // Comparisons.
    let _ = KnownBits::eq(&a, &b);
    let _ = KnownBits::ne(&a, &b);
    let _ = KnownBits::ugt(&a, &b);
    let _ = KnownBits::uge(&a, &b);
    let _ = KnownBits::ult(&a, &b);
    let _ = KnownBits::ule(&a, &b);
    let _ = KnownBits::sgt(&a, &b);
    let _ = KnownBits::sge(&a, &b);
    let _ = KnownBits::slt(&a, &b);
    let _ = KnownBits::sle(&a, &b);

    // Unary shape operations.
    let _ = a.abs();
    let _ = a.abs_with_int_min_poison(true);
    let _ = a.reduce_add(2);
    let _ = a.byte_swap();
    let _ = a.reverse_bits();
    let _ = a.blsi();
    let _ = a.blsmsk();

    // The operators, each through the std trait that spells it.
    let _ = &a & &b;
    let _ = &a | &b;
    let _ = &a ^ &b;
    let _ = &a << 2;
    let _ = &a >> 2;
    let _ = a == b;
    let _ = a != b;
    let mut assigning = a.clone();
    assigning &= &b;
    assigning |= &b;
    assigning ^= &b;
    assigning <<= 1;
    assigning >>= 1;

    // Debug printing: `print` is Display, `dump` is the derived Debug.
    let _ = format!("{a}");
    let _ = format!("{a:?}");
}

/// Every entry in [`MODELED_VALUE_TRACKING`] resolves to a real public item.
///
/// Naming each as a value is the whole claim: the compiler must resolve the
/// path and instantiate the signature, so a rename or removal stops this file
/// compiling. No upstream counterpart — see the module docs.
#[test]
fn exercises_every_modeled_value_tracking_entry_point() {
    use llvmkit_ir::{
        BytewiseValue, ConstantDataArraySlice, MinMaxIntrinsic, MinMaxKind, MinMaxOperation,
        SelectPatternFlavor, SelectPatternMatch, SelectPatternNaNBehavior, SelectPatternResult,
        argument_aliasing_to_returned_pointer, can_convert_to_min_or_max_intrinsic,
        collect_possible_values, find_alloca_for_value, find_inserted_value,
        get_constant_data_array_info, get_constant_string_info, get_select_pattern,
        get_string_length, get_underlying_object, get_underlying_object_aggressive,
        get_underlying_objects, get_underlying_objects_for_code_gen, is_bytewise_value,
        is_intrinsic_returning_pointer_aliasing_argument_without_capturing,
        match_decomposed_select_pattern, match_select_pattern, only_used_by_lifetime_markers,
        only_used_by_lifetime_markers_or_droppable_instructions, pointer_base_with_constant_offset,
        strip_null_test,
    };
    use llvmkit_ir::{
        DynBrand, SpeculationOptions, block_transfers_execution_to_successor, can_create_poison,
        instructions_transfer_execution_to_successor, intrinsic_propagates_poison,
        is_assume_like_intrinsic, is_guaranteed_to_execute_for_every_iteration,
        is_guaranteed_to_transfer_execution_to_successor, is_not_cross_lane_operation,
        is_safe_to_speculatively_execute, is_safe_to_speculatively_execute_with_opcode,
        is_safe_to_speculatively_execute_with_variable_replaced, may_have_non_def_use_dependency,
        must_execute_ub_if_poison_on_path_to, must_trigger_ub, program_undefined_if_poison,
        program_undefined_if_undef_or_poison,
    };
    use llvmkit_ir::{
        can_create_undef_or_poison, compute_constant_range,
        compute_constant_range_including_known_bits, compute_known_bits,
        compute_max_significant_bits, compute_num_sign_bits, compute_overflow_for_signed_add,
        compute_overflow_for_signed_mul, compute_overflow_for_signed_sub,
        compute_overflow_for_unsigned_add, compute_overflow_for_unsigned_mul,
        compute_overflow_for_unsigned_sub, have_no_common_bits_set, implies_poison,
        is_known_inversion, is_known_negation, is_known_negative, is_known_non_equal,
        is_known_non_negative, is_known_non_zero, is_known_not_poison, is_known_not_undef,
        is_known_not_undef_or_poison, is_known_one, is_known_positive,
        is_known_to_be_a_power_of_two, is_known_zero, is_only_used_in_zero_comparison,
        is_only_used_in_zero_equality_comparison, is_sign_bit_check, masked_value_is_zero,
        propagates_poison,
    };

    let _compute_known_bits = compute_known_bits::<DynBrand>;
    let _compute_num_sign_bits = compute_num_sign_bits::<DynBrand>;
    let _compute_max_significant_bits = compute_max_significant_bits::<DynBrand>;
    let _can_create_poison = can_create_poison::<DynBrand>;
    let _can_create_undef_or_poison = can_create_undef_or_poison::<DynBrand>;
    let _implies_poison = implies_poison::<DynBrand>;
    let _is_known_not_poison = is_known_not_poison::<DynBrand>;
    let _propagates_poison = propagates_poison::<DynBrand>;
    let _compute_constant_range = compute_constant_range::<DynBrand>;
    let _compute_constant_range_including_known_bits =
        compute_constant_range_including_known_bits::<DynBrand>;
    let _compute_overflow_for_signed_add = compute_overflow_for_signed_add::<DynBrand>;
    let _compute_overflow_for_signed_mul = compute_overflow_for_signed_mul::<DynBrand>;
    let _compute_overflow_for_signed_sub = compute_overflow_for_signed_sub::<DynBrand>;
    let _compute_overflow_for_unsigned_add = compute_overflow_for_unsigned_add::<DynBrand>;
    let _compute_overflow_for_unsigned_mul = compute_overflow_for_unsigned_mul::<DynBrand>;
    let _compute_overflow_for_unsigned_sub = compute_overflow_for_unsigned_sub::<DynBrand>;
    let _is_known_non_zero = is_known_non_zero::<DynBrand>;
    let _masked_value_is_zero = masked_value_is_zero::<DynBrand>;
    let _have_no_common_bits_set = have_no_common_bits_set::<DynBrand>;
    let _is_known_not_undef = is_known_not_undef::<DynBrand>;
    let _is_known_not_undef_or_poison = is_known_not_undef_or_poison::<DynBrand>;
    let _is_known_inversion = is_known_inversion::<DynBrand>;
    let _is_known_negation = is_known_negation::<DynBrand>;
    let _is_known_negative = is_known_negative::<DynBrand>;
    let _is_known_non_equal = is_known_non_equal::<DynBrand>;
    let _is_known_non_negative = is_known_non_negative::<DynBrand>;
    let _is_known_positive = is_known_positive::<DynBrand>;
    let _is_known_to_be_a_power_of_two = is_known_to_be_a_power_of_two::<DynBrand>;
    let _is_only_used_in_zero_comparison = is_only_used_in_zero_comparison::<DynBrand>;
    let _is_only_used_in_zero_equality_comparison =
        is_only_used_in_zero_equality_comparison::<DynBrand>;
    let _is_sign_bit_check = is_sign_bit_check;
    // A type rather than a function: `OverflowResult` is what the six
    // `compute_overflow_for_*` entry points return.
    let _overflow_result = llvmkit_ir::OverflowResult::MayOverflow;

    // Speculation safety and UB reachability (tranche 6). Upstream declares
    // `isGuaranteedToTransferExecutionToSuccessor` four times over — for an
    // instruction, a block, and two spellings of an instruction range — which
    // is one ledger row and three functions here.
    let _is_safe_to_speculatively_execute = is_safe_to_speculatively_execute::<DynBrand>;
    let _is_safe_to_speculatively_execute_with_opcode =
        is_safe_to_speculatively_execute_with_opcode::<DynBrand>;
    let _is_safe_to_speculatively_execute_with_variable_replaced =
        is_safe_to_speculatively_execute_with_variable_replaced::<DynBrand>;
    let _is_guaranteed_to_transfer_execution_to_successor =
        is_guaranteed_to_transfer_execution_to_successor::<DynBrand>;
    let _block_transfers_execution_to_successor =
        block_transfers_execution_to_successor::<DynBrand>;
    let _instructions_transfer_execution_to_successor =
        instructions_transfer_execution_to_successor::<
            DynBrand,
            Vec<llvmkit_ir::InstructionView<'static, DynBrand>>,
        >;
    let _is_guaranteed_to_execute_for_every_iteration =
        is_guaranteed_to_execute_for_every_iteration::<DynBrand>;
    let _may_have_non_def_use_dependency = may_have_non_def_use_dependency::<DynBrand>;
    let _must_trigger_ub = must_trigger_ub::<DynBrand>;
    let _must_execute_ub_if_poison_on_path_to = must_execute_ub_if_poison_on_path_to::<DynBrand>;
    let _program_undefined_if_poison = program_undefined_if_poison::<DynBrand>;
    let _program_undefined_if_undef_or_poison = program_undefined_if_undef_or_poison::<DynBrand>;
    let _is_assume_like_intrinsic = is_assume_like_intrinsic::<DynBrand>;
    let _is_not_cross_lane_operation = is_not_cross_lane_operation::<DynBrand>;
    let _intrinsic_propagates_poison = intrinsic_propagates_poison;
    // The options record `isSafeToSpeculativelyExecute`'s two defaulted `bool`
    // parameters; `Default` is upstream's no-argument call.
    let _speculation_options = SpeculationOptions::new()
        .with_variable_info(false)
        .ignoring_ub_implying_attrs(false);
    let _default_transfer_scan_limit = llvmkit_ir::DEFAULT_TRANSFER_SCAN_LIMIT;

    // Pointer and object analysis (tranche 5).
    let _get_underlying_object = get_underlying_object::<DynBrand>;
    let _get_underlying_object_aggressive = get_underlying_object_aggressive::<DynBrand>;
    let _get_underlying_objects = get_underlying_objects::<DynBrand>;
    let _get_underlying_objects_for_code_gen = get_underlying_objects_for_code_gen::<DynBrand>;
    let _pointer_base_with_constant_offset = pointer_base_with_constant_offset::<DynBrand>;
    let _find_alloca_for_value = find_alloca_for_value::<DynBrand>;
    let _only_used_by_lifetime_markers = only_used_by_lifetime_markers::<DynBrand>;
    let _only_used_by_lifetime_markers_or_droppable_instructions =
        only_used_by_lifetime_markers_or_droppable_instructions::<DynBrand>;
    let _argument_aliasing_to_returned_pointer = argument_aliasing_to_returned_pointer::<DynBrand>;
    let _is_intrinsic_returning_pointer_aliasing_argument_without_capturing =
        is_intrinsic_returning_pointer_aliasing_argument_without_capturing::<DynBrand>;
    let _get_constant_data_array_info = get_constant_data_array_info::<DynBrand>;
    let _get_constant_string_info = get_constant_string_info::<DynBrand>;
    let _get_string_length = get_string_length::<DynBrand>;
    let _is_bytewise_value = is_bytewise_value::<DynBrand>;
    let _find_inserted_value = find_inserted_value::<DynBrand>;
    // A type rather than a function: what `get_constant_data_array_info`
    // returns, and the window `get_string_length` reads through.
    let _slice_accessors = (
        ConstantDataArraySlice::<DynBrand>::array,
        ConstantDataArraySlice::<DynBrand>::offset,
        ConstantDataArraySlice::<DynBrand>::len,
        ConstantDataArraySlice::<DynBrand>::is_empty,
        ConstantDataArraySlice::<DynBrand>::moved,
        ConstantDataArraySlice::<DynBrand>::element,
    );
    let _max_lookup_search_depth = llvmkit_ir::MAX_LOOKUP_SEARCH_DEPTH;

    // Tranche 7 — floating-point classification.
    let _compute_known_fp_class = (
        llvmkit_ir::compute_known_fp_class::<DynBrand>,
        llvmkit_ir::compute_known_fp_class_all::<DynBrand>,
        llvmkit_ir::compute_known_fp_class_with_flags::<DynBrand>,
    );
    let _fp_predicates = (
        llvmkit_ir::is_known_never_nan::<DynBrand>,
        llvmkit_ir::is_known_never_infinity::<DynBrand>,
        llvmkit_ir::is_known_never_infinity_or_nan::<DynBrand>,
        llvmkit_ir::cannot_be_negative_zero::<DynBrand>,
        llvmkit_ir::cannot_be_ordered_less_than_zero::<DynBrand>,
        llvmkit_ir::compute_known_fp_sign_bit::<DynBrand>,
    );
    let _sign_bit_indifference = (
        llvmkit_ir::can_ignore_sign_bit_of_zero::<DynBrand>,
        llvmkit_ir::can_ignore_sign_bit_of_nan::<DynBrand>,
    );
    let _adjust_known_fp_class_for_select_arm =
        llvmkit_ir::adjust_known_fp_class_for_select_arm::<DynBrand>;
    let _analyze_known_bits_from_and_xor_or =
        llvmkit_ir::analyze_known_bits_from_and_xor_or::<DynBrand>;

    // Tranche 8 — assumptions and implied conditions.
    let _compute_known_bits_from_context = llvmkit_ir::compute_known_bits_from_context::<DynBrand>;
    let _adjust_known_bits_for_select_arm =
        llvmkit_ir::adjust_known_bits_for_select_arm::<DynBrand>;
    let _is_valid_assume_for_context = llvmkit_ir::is_valid_assume_for_context::<DynBrand>;
    let _will_not_free_between = llvmkit_ir::will_not_free_between::<DynBrand>;
    let _find_values_affected_by_condition =
        llvmkit_ir::find_values_affected_by_condition::<DynBrand, fn(llvmkit_ir::Value<DynBrand>)>;
    // Both overloads of each, as upstream declares them.
    let _is_implied_condition = (
        llvmkit_ir::is_implied_condition::<DynBrand>,
        llvmkit_ir::is_implied_condition_decomposed::<DynBrand>,
    );
    let _is_implied_by_dom_condition = (
        llvmkit_ir::is_implied_by_dom_condition::<DynBrand>,
        llvmkit_ir::is_implied_by_dom_condition_decomposed::<DynBrand>,
    );
    // The condition-injection vehicle `computeKnownBitsFromContext` reads.
    let _cond_context = (
        llvmkit_ir::CondContext::<DynBrand>::new,
        llvmkit_ir::CondContext::<DynBrand>::inverted,
        llvmkit_ir::CondContext::<DynBrand>::affects,
    );
    let _assumption_caches = (
        llvmkit_ir::AssumptionCache::new::<DynBrand>,
        llvmkit_ir::AssumptionCache::assumptions_for::<DynBrand>,
        llvmkit_ir::DomConditionCache::register_branch::<DynBrand>,
        llvmkit_ir::DomConditionCache::conditions_for::<DynBrand>,
    );
    // Not in the table — the answer shape of `isBytewiseValue`, which upstream
    // spells as a `Value *` because it can mint the constant.
    let _bytewise_value = BytewiseValue::<DynBrand>::AnyByte;

    // Residue ported 2026-08-04.
    let _collect_possible_values = collect_possible_values::<DynBrand>;
    let _strip_null_test = strip_null_test::<DynBrand>;

    // The min/max vocabulary. `getInverseMinMaxIntrinsic` spans both halves of
    // the family, so all three spellings of its ledger row are named here.
    let _min_max_vocabulary = (
        SelectPatternFlavor::inverse_min_max,
        SelectPatternFlavor::min_max_intrinsic,
        SelectPatternFlavor::min_max_limit,
        SelectPatternFlavor::min_max_predicate,
        MinMaxOperation::inverse,
        MinMaxIntrinsic::inverse,
        MinMaxKind::inverse,
    );

    // The flavour classification and the record it answers with (tranche 4a).
    let _get_select_pattern: fn(_, SelectPatternNaNBehavior, bool) -> SelectPatternResult =
        get_select_pattern;
    let _select_pattern_result = SelectPatternResult::unknown;

    // Select-pattern matching (tranche 4b).
    let _match_select_pattern = match_select_pattern::<DynBrand>;
    let _match_decomposed_select_pattern = match_decomposed_select_pattern::<DynBrand>;
    let _can_convert_to_min_or_max_intrinsic =
        can_convert_to_min_or_max_intrinsic::<DynBrand, Vec<llvmkit_ir::Value<'static, DynBrand>>>;
    // Not in the table — the record `matchSelectPattern` fills in through its
    // `LHS` / `RHS` / `CastOp` out-parameters.
    let _select_pattern_match = |m: SelectPatternMatch<'static, DynBrand>| m.result;
    // Not in the table — llvmkit-specific conveniences with no upstream entry
    // point of their own.
    let _is_known_zero = is_known_zero::<DynBrand>;
    let _is_known_one = is_known_one::<DynBrand>;
}

/// The ledger tables stay readable: sorted, duplicate-free, every gap carries a
/// reason, and no symbol is both modeled and a gap.
///
/// No upstream counterpart — see the module docs.
#[test]
fn ledger_tables_are_consistent() {
    for (label, modeled, gaps) in [
        ("KnownBits", MODELED_KNOWN_BITS, KNOWN_BITS_GAPS),
        ("ValueTracking", MODELED_VALUE_TRACKING, VALUE_TRACKING_GAPS),
    ] {
        assert!(!modeled.is_empty(), "{label}: modeled table is empty");

        let modeled_names: Vec<&str> = modeled.iter().map(|(upstream, _)| *upstream).collect();
        let unique: BTreeSet<&str> = modeled_names.iter().copied().collect();
        assert_eq!(
            unique.len(),
            modeled_names.len(),
            "{label}: duplicate entry in the modeled table"
        );

        let mut sorted = modeled_names.clone();
        sorted.sort_unstable();
        assert_eq!(
            modeled_names, sorted,
            "{label}: modeled table must stay sorted by upstream name"
        );

        for (upstream, llvmkit) in modeled {
            assert!(
                !llvmkit.trim().is_empty(),
                "{label}: `{upstream}` records no llvmkit spelling"
            );
        }

        for (gap, reason) in gaps {
            assert!(
                !unique.contains(gap),
                "{label}: `{gap}` is listed as both modeled and a gap"
            );
            assert!(
                !reason.trim().is_empty(),
                "{label}: `{gap}` has no recorded reason"
            );
        }
    }

    assert!(
        !DERIVED_FROM_LLVM.is_empty(),
        "the gap lists must name the LLVM release they were derived from"
    );

    // The private-upstream list is neither modeled nor a gap; it must not
    // collide with either.
    let modeled: BTreeSet<&str> = MODELED_KNOWN_BITS.iter().map(|(u, _)| *u).collect();
    for (private, llvmkit) in KNOWN_BITS_PRIVATE_UPSTREAM {
        assert!(
            !modeled.contains(private),
            "`{private}` is private upstream but listed as modeled public API"
        );
        assert!(
            !llvmkit.trim().is_empty(),
            "`{private}` records no llvmkit location"
        );
    }
}

/// Every `ValueTracking.h` entry point is either modeled or recorded as a gap.
///
/// The `KnownBits` half asserts its gap list is *empty*; this one cannot,
/// because most of `ValueTracking.h` is genuinely unported. What it asserts
/// instead is that nothing is **unaccounted for** — modeled plus gaps has to
/// equal the audited surface size, so a symbol cannot be neither.
///
/// Before 2026-08-03 that property did not hold and nothing noticed: the gap
/// table was seven prose rows keyed by family, and 47 of the header's 101
/// entry points appeared in neither table. See [`VALUE_TRACKING_GAPS`].
///
/// No upstream counterpart; see the module docs.
#[test]
fn value_tracking_surface_is_accounted_for() {
    let (_, audited) = VALUE_TRACKING_SURFACE_AUDITED;
    assert_eq!(
        MODELED_VALUE_TRACKING.len() + VALUE_TRACKING_GAPS.len(),
        audited,
        "modeled ({}) + gaps ({}) no longer covers the {audited} entry points \
         ValueTracking.h declares; re-run the enumeration recorded on \
         VALUE_TRACKING_SURFACE_AUDITED and reconcile",
        MODELED_VALUE_TRACKING.len(),
        VALUE_TRACKING_GAPS.len(),
    );

    // A symbol must not be both claimed and disclaimed.
    let modeled: BTreeSet<&str> = MODELED_VALUE_TRACKING.iter().map(|(u, _)| *u).collect();
    let private: BTreeSet<&str> = VALUE_TRACKING_PRIVATE_UPSTREAM
        .iter()
        .map(|(u, _)| *u)
        .collect();
    for (upstream, _) in VALUE_TRACKING_GAPS {
        assert!(
            !modeled.contains(upstream),
            "{upstream} is listed as both modeled and a gap"
        );
    }
    for name in &private {
        assert!(
            !modeled.contains(name),
            "{name} is internal to ValueTracking.cpp and must not sit in the \
             modeled table, which tracks the header"
        );
    }
}

/// Every row of [`MODELED_KNOWN_BITS`] is actually reached by
/// `exercises_every_modeled_known_bits_operation`.
///
/// That test's name is a claim about the table, but nothing linked the two:
/// the table is data and the exercise fn is code, so a row could be added with
/// no call to match and the "modeled" column would still look enforced. This
/// reads the file's own source and checks each row against the body.
///
/// Rows whose llvmkit column names a trait impl have no `.name(` call site;
/// [`KNOWN_BITS_TRAIT_PROBES`] says what syntax to look for instead.
///
/// No upstream counterpart; see the module docs.
#[test]
fn every_modeled_known_bits_row_is_exercised() {
    // Reading our own source is the only way to tie the two together: the
    // table is data, the exercise fn is code, and nothing else connects them.
    const SELF: &str = include_str!("value_tracking_parity.rs");

    let body = function_body(SELF, "fn exercises_every_modeled_known_bits_operation");
    let body = strip_line_comments(&body);

    let mut unexercised: Vec<String> = Vec::new();
    for (upstream, llvmkit) in MODELED_KNOWN_BITS {
        if let Some((_, probes)) = KNOWN_BITS_TRAIT_PROBES
            .iter()
            .find(|(column, _)| column == llvmkit)
        {
            for probe in *probes {
                if !body.contains(probe) {
                    unexercised.push(format!("{upstream} ({llvmkit}): no `{probe}`"));
                }
            }
            continue;
        }
        for name in llvmkit.split('/').map(str::trim) {
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{upstream}: llvmkit column {llvmkit:?} is neither a method name nor a \
                 KNOWN_BITS_TRAIT_PROBES entry"
            );
            if !body.contains(&format!(".{name}(")) && !body.contains(&format!("::{name}(")) {
                unexercised.push(format!("{upstream} ({llvmkit}): no call to `{name}(`"));
            }
        }
    }

    assert!(
        unexercised.is_empty(),
        "exercises_every_modeled_known_bits_operation does not reach {} of {} rows:\n  {}",
        unexercised.len(),
        MODELED_KNOWN_BITS.len(),
        unexercised.join("\n  ")
    );
}

/// Every row of [`MODELED_VALUE_TRACKING`] is actually reached by
/// `exercises_every_modeled_value_tracking_entry_point`.
///
/// The `KnownBits` half of this ledger has had
/// [`every_modeled_known_bits_row_is_exercised`] since the 2026-08-03 audit;
/// the `ValueTracking` half had nothing, while the module header claimed the
/// guarantee for both tables. It had already drifted by the time this landed:
/// `getInverseMinMaxFlavor`, `getMinMaxIntrinsic`, `getMinMaxLimit` and
/// `getMinMaxPred` sat in the modeled table with the exercise fn naming none
/// of them.
///
/// **The probe differs from the `KnownBits` one because the shapes differ.**
/// A `KnownBits` row is a method, so `.name(` is the call site to look for. A
/// `ValueTracking` row is mostly a free function or a type, and the exercise fn
/// *names* those as values rather than calling them — a call would need a
/// module, a brand and operands per row. So this looks for the identifier path
/// at a word boundary, which is what stops `compute_known_bits` being satisfied
/// by `compute_known_bits_from_context`, or `propagates_poison` by
/// `intrinsic_propagates_poison`.
///
/// `use` declarations are stripped first: they sit inside the fn body, and
/// importing a name is not exercising it. In practice `-D warnings` already
/// makes an unused import a build failure, so this is belt-and-braces.
///
/// No upstream counterpart; see the module docs.
#[test]
fn every_modeled_value_tracking_row_is_exercised() {
    // Reading our own source is the only way to tie the two together: the
    // table is data, the exercise fn is code, and nothing else connects them.
    const SELF: &str = include_str!("value_tracking_parity.rs");

    let body = function_body(
        SELF,
        "fn exercises_every_modeled_value_tracking_entry_point",
    );
    let body = strip_use_declarations(&strip_line_comments(&body));

    let mut unexercised: Vec<String> = Vec::new();
    for (upstream, llvmkit) in MODELED_VALUE_TRACKING {
        for path in llvmkit.split('/').map(str::trim) {
            if !names_path(&body, path) {
                unexercised.push(format!("{upstream} ({llvmkit}): `{path}` is never named"));
            }
        }
    }

    assert!(
        unexercised.is_empty(),
        "exercises_every_modeled_value_tracking_entry_point does not reach {} of {} rows:\n  {}",
        unexercised.len(),
        MODELED_VALUE_TRACKING.len(),
        unexercised.join("\n  ")
    );
}

/// The `{ .. }` body of the item whose declaration starts with `header`.
fn function_body(source: &str, header: &str) -> String {
    let start = source
        .find(header)
        .unwrap_or_else(|| panic!("{header} not found in source"));
    let mut depth = 0usize;
    let mut seen_open = false;
    for (offset, ch) in source[start..].char_indices() {
        match ch {
            '{' => {
                depth += 1;
                seen_open = true;
            }
            '}' => {
                depth -= 1;
                if seen_open && depth == 0 {
                    return source[start..start + offset + 1].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced braces after {header}");
}

/// Drop `//` comments so a name mentioned in prose does not count as a call.
fn strip_line_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Drop `use` items, so importing a name does not count as exercising it.
///
/// `exercises_every_modeled_value_tracking_entry_point` keeps its imports
/// inside the fn, so they are part of the body this file reads back.
fn strip_use_declarations(source: &str) -> String {
    let mut kept = String::new();
    let mut inside = false;
    for line in source.lines() {
        let trimmed = line.trim();
        if inside || trimmed.starts_with("use ") {
            inside = !trimmed.ends_with(';');
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    kept
}

/// Whether `body` names `path` as a whole identifier path rather than as a
/// fragment of a longer one.
///
/// A leading `::` is fine — `llvmkit_ir::is_known_negation` names
/// `is_known_negation` — but an identifier character on either side is not, so
/// `intrinsic_propagates_poison` does not stand in for `propagates_poison`.
fn names_path(body: &str, path: &str) -> bool {
    let is_identifier_char = |c: char| c.is_alphanumeric() || c == '_';
    let mut from = 0usize;
    while let Some(offset) = body[from..].find(path) {
        let start = from + offset;
        let end = start + path.len();
        let before = body[..start].chars().next_back();
        let after = body[end..].chars().next();
        if !before.is_some_and(is_identifier_char) && !after.is_some_and(is_identifier_char) {
            return true;
        }
        from = start + 1;
    }
    false
}

/// Every operation `KnownBits.h` declares public is modeled.
///
/// **What this test can and cannot prove.** It compares two hand-maintained
/// tables against each other. It cannot read `KnownBits.h` — `orig_cpp/` is
/// gitignored, so it does not exist in CI — which means it detects a
/// *recorded* gap, never an *unrecorded* one. Adding a `KNOWN_BITS_GAPS`
/// entry fails here and forces the gap to be acknowledged; upstream growing a
/// method that nobody notices does not.
///
/// That is not hypothetical. The tables were built by grepping the header for
/// an identifier followed by `(`, which silently skipped all seven operators,
/// and `operator<<=` / `operator>>=` were unmodeled for as long as this test
/// reported the surface closed. Closing the surface is a periodic manual
/// enumeration — [`KNOWN_BITS_SURFACE_AUDITED`] records when it last ran and
/// how to reproduce it.
///
/// What *is* enforced here: the tables sum to the audited size, and
/// `every_modeled_known_bits_row_is_exercised` proves each row reaches real
/// code.
///
/// No upstream counterpart; see the module docs.
#[test]
fn known_bits_public_surface_is_complete() {
    assert!(
        KNOWN_BITS_GAPS.is_empty(),
        "KnownBits.h is no longer fully modeled: {:?}",
        KNOWN_BITS_GAPS
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
    );

    // The tables must at least agree with each other and with the audit that
    // produced them. This does not make the assertion above self-checking —
    // see the docs — but it does catch a row being added to one table and
    // forgotten in the count.
    let (_, audited) = KNOWN_BITS_SURFACE_AUDITED;
    assert_eq!(
        MODELED_KNOWN_BITS.len() + KNOWN_BITS_GAPS.len(),
        audited,
        "the modeled + gap tables no longer add up to the audited surface size; \
         re-run the enumeration and update KNOWN_BITS_SURFACE_AUDITED"
    );
}
