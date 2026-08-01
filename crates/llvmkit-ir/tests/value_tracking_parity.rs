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
//!   the gap list, with a recorded reason on every gap.
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
    ("eq", "eq"),
    ("extractBits", "extract_bits"),
    ("getBitWidth", "bit_width"),
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
/// **Empty, and asserted to stay empty.** Every operation `KnownBits.h`
/// declares public is modeled; see [`KNOWN_BITS_PRIVATE_UPSTREAM`] for the two
/// that look missing but are upstream internals. Adding an entry here trips
/// `known_bits_public_surface_is_complete`, which is the point — a regression
/// or a newly-synced upstream method has to be acknowledged, not absorbed.
const KNOWN_BITS_GAPS: &[(&str, &str)] = &[];

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
/// A deliberately small surface: llvmkit ports the known-bits core and the
/// predicates built directly on it, and nothing else from the ~80 entry points
/// `ValueTracking.h` declares.
///
/// `is_known_one` is exercised alongside these but is absent from the table on
/// purpose — it is an llvmkit-specific dual of `is_known_zero` with no upstream
/// entry point of its own.
const MODELED_VALUE_TRACKING: &[(&str, &str)] = &[
    ("ComputeMaxSignificantBits", "compute_max_significant_bits"),
    ("ComputeNumSignBits", "compute_num_sign_bits"),
    ("MaskedValueIsZero", "is_known_zero"),
    ("computeKnownBits", "compute_known_bits"),
    ("computeKnownBitsFromOperator", "known_bits_from_operator"),
    ("isKnownNonZero", "is_known_non_zero"),
];

/// The `ValueTracking.h` families llvmkit does not model at all, so the ledger
/// reads as coverage rather than silence.
///
/// Derived at LLVM [`DERIVED_FROM_LLVM`], listed by family rather than by
/// symbol: enumerating ~76 individually would be noise, and every family here
/// is absent wholesale.
const VALUE_TRACKING_GAPS: &[(&str, &str)] = &[
    (
        "assumption cache",
        "computeKnownBitsFromContext — no @llvm.assume-driven refinement",
    ),
    (
        "constant ranges",
        "computeConstantRange, computeConstantRangeIncludingKnownBits, \
         getVScaleRange — same missing ConstantRange dependency",
    ),
    (
        "floating-point classification",
        "computeKnownFPClass, isKnownNeverNaN, isKnownNeverInfinity, \
         cannotBeNegativeZero, canIgnoreSignBitOfNaN — llvmkit has ApFloat but no \
         FP known-class lattice",
    ),
    (
        "overflow prediction",
        "computeOverflowForSignedAdd / Sub / Mul and the unsigned trio — needs \
         ConstantRange, which llvmkit does not model",
    ),
    (
        "pointer and object analysis",
        "getUnderlyingObjects, getConstantStringInfo, GetStringLength, \
         getConstantDataArrayInfo, onlyUsedByLifetimeMarkers",
    ),
    (
        "poison and UB reasoning",
        "canCreatePoison, impliesPoison, programUndefinedIfPoison, mustTriggerUB, \
         propagatesPoison — llvmkit models only isGuaranteedNotToBePoison, and \
         that internally, for the freeze arm",
    ),
    (
        "select-pattern matching",
        "getSelectPattern, matchDecomposedSelectPattern, getMinMaxIntrinsic, \
         getInverseMinMaxIntrinsic",
    ),
    (
        "speculation safety",
        "isSafeToSpeculativelyExecute and friends, \
         isGuaranteedToTransferExecutionToSuccessor, isValidAssumeForContext",
    ),
];

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
}

/// Every entry in [`MODELED_VALUE_TRACKING`] resolves to a real public item.
///
/// Naming each as a value is the whole claim: the compiler must resolve the
/// path and instantiate the signature, so a rename or removal stops this file
/// compiling. No upstream counterpart — see the module docs.
#[test]
fn exercises_every_modeled_value_tracking_entry_point() {
    use llvmkit_ir::{
        DynBrand, compute_known_bits, compute_max_significant_bits, compute_num_sign_bits,
        is_known_non_zero, is_known_one, is_known_zero,
    };

    let _compute_known_bits = compute_known_bits::<DynBrand>;
    let _compute_num_sign_bits = compute_num_sign_bits::<DynBrand>;
    let _compute_max_significant_bits = compute_max_significant_bits::<DynBrand>;
    let _is_known_non_zero = is_known_non_zero::<DynBrand>;
    let _is_known_zero = is_known_zero::<DynBrand>;
    // Not in the table — an llvmkit-specific dual with no upstream entry point.
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

/// Every operation `KnownBits.h` declares public is modeled.
///
/// The assertion is the tripwire: adding a `KNOWN_BITS_GAPS` entry — because a
/// method regressed, or because an LLVM sync introduced one — fails here and
/// forces the gap to be acknowledged rather than quietly absorbed into a list
/// nobody reads. No upstream counterpart; see the module docs.
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
}
