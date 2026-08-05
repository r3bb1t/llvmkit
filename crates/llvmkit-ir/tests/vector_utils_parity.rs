//! Parity ledger for the `VectorUtils` surface.
//!
//! Anchors: `llvm/include/llvm/Analysis/VectorUtils.h` and
//! `llvm/lib/Analysis/VectorUtils.cpp`.
//!
//! **No upstream counterpart.** A coverage ledger is an artifact of being a
//! reimplementation; LLVM has nothing to mirror it against. This follows
//! `value_tracking_parity.rs`, whose module docs explain the design at length.
//!
//! # What has teeth, and what does not
//!
//! `orig_cpp/` is gitignored, so this file cannot read `VectorUtils.cpp` at
//! test time. The guarantee splits three ways, exactly as the ValueTracking
//! ledger's does:
//!
//! - **Enforced by the compiler.** [`exercises_every_modeled_entry_point`]
//!   names every modeled function as a value, so the compiler must resolve
//!   each path and instantiate each signature. A rename or removal stops this
//!   file compiling, so the modeled column cannot silently become a lie.
//! - **Enforced at run time.** The two tables are checked for the properties a
//!   ledger needs to stay readable — sorted, duplicate-free, disjoint, every
//!   gap carrying a reason — and for the total, which is the claim the module
//!   docs make out loud.
//! - **Maintained by hand.** The gap list is a human record. Nothing here can
//!   notice that upstream grew a function; that is the LLVM sync's job, which
//!   should re-derive both tables and reconcile. Hence
//!   [`DERIVED_FROM_LLVM`].
//!
//! # Why the totals look off by one
//!
//! `widenShuffleMaskElts` is one upstream *name* with two overloads, and
//! llvmkit spells them as two functions because they are two rules. The tables
//! count names, so the name total is 37 while the function count is 38.

use std::collections::BTreeSet;

/// The LLVM release the tables below were derived from. Bump it in the same
/// commit that reconciles them against a newer `VectorUtils.cpp`.
const DERIVED_FROM_LLVM: &str = "22.1.4";

/// Every `llvm::`-qualified function `VectorUtils.cpp` defines, counted by
/// name.
///
/// Re-derive with
/// `grep -oE "\bllvm::[a-zA-Z_][a-zA-Z0-9_]*\(" … | sort -u`, discarding
/// `bit_ceil` and `bit_width` — those are calls into `ADT/bit.h`, not
/// definitions. An earlier count was wrong twice because a `grep` anchoring
/// the return type and the name to one line silently skips every definition
/// whose return type wraps.
const UPSTREAM_ENTRY_POINT_COUNT: usize = 37;

/// `VectorUtils.cpp` functions llvmkit models, as
/// `(upstream name, llvmkit name)`. Sorted by upstream name.
const MODELED: &[(&str, &str)] = &[
    ("createInterleaveMask", "create_interleave_mask"),
    ("createReplicatedMask", "create_replicated_mask"),
    ("createSequentialMask", "create_sequential_mask"),
    ("createStrideMask", "create_stride_mask"),
    ("createUnaryMask", "create_unary_mask"),
    ("findScalarElement", "find_scalar_element"),
    (
        "getDeinterleaveIntrinsicFactor",
        "deinterleave_intrinsic_factor",
    ),
    (
        "getHorizDemandedEltsForFirstOperand",
        "horizontal_demanded_elements_for_first_operand",
    ),
    (
        "getInterleaveIntrinsicFactor",
        "interleave_intrinsic_factor",
    ),
    ("getShuffleDemandedElts", "shuffle_demanded_elements"),
    (
        "getShuffleMaskWithWidestElts",
        "shuffle_mask_with_widest_elements",
    ),
    ("getSplatIndex", "splat_index"),
    ("getSplatValue", "get_splat_value"),
    ("isMaskedSlidePair", "masked_slide_pair"),
    ("isSplatValue", "is_splat_value"),
    ("isTriviallyScalarizable", "is_trivially_scalarizable"),
    ("isTriviallyVectorizable", "is_trivially_vectorizable"),
    (
        "isVectorIntrinsicWithStructReturnOverloadAtField",
        "is_vector_intrinsic_with_struct_return_overload_at_field",
    ),
    (
        "maskContainsAllOneOrUndef",
        "mask_contains_all_one_or_undefined",
    ),
    ("maskIsAllOneOrUndef", "mask_is_all_one_or_undefined"),
    ("maskIsAllZeroOrUndef", "mask_is_all_zero_or_undefined"),
    ("narrowShuffleMaskElts", "narrow_shuffle_mask_elements"),
    (
        "possiblyDemandedEltsInMask",
        "possibly_demanded_elements_in_mask",
    ),
    ("scaleShuffleMaskElts", "scale_shuffle_mask_elements"),
    // One upstream name, two llvmkit functions — see the module docs.
    (
        "widenShuffleMaskElts",
        "widen_shuffle_mask_elements / widen_shuffle_mask_elements_in_pairs",
    ),
];

/// `VectorUtils.cpp` functions llvmkit does **not** model, each with the thing
/// that blocks it. Sorted by upstream name.
///
/// Two of these are marked `[permanent]`: llvmkit models no target, and code
/// generation and target backends are excluded by charter rather than merely
/// unfinished.
///
/// **An earlier revision of this table recorded eight of these as blocked on
/// "needs `Intrinsic::ID`", and that reason was wrong.** `IntrinsicId` has been
/// public, generated-backed and whole-space throughout; what is `pub(crate)` is
/// `IntrinsicSemantic`, a convenience enum over a 31-name *subset*, and the
/// reason conflated the two. Five of the eight are now modeled. The three that
/// remain are blocked on things that are actually missing, named below.
const GAPS: &[(&str, &str)] = &[
    (
        "computeMinimumValueSizes",
        "[permanent] takes a TargetTransformInfo; llvmkit models no target",
    ),
    ("concatenateVectors", "needs an IRBuilder"),
    ("createBitMaskForGaps", "needs an IRBuilder"),
    ("getDeinterleavedVectorType", "needs IntrinsicInst"),
    ("getMetadataToPropagate", "needs metadata modeling"),
    (
        "getVectorIntrinsicIDForCall",
        "needs getIntrinsicForCallSite, whose library-function half wants \
         TargetLibraryInfo",
    ),
    ("intersectAccessGroups", "needs metadata modeling"),
    (
        "isVectorIntrinsicWithOverloadTypeAtArg",
        "needs VPCastIntrinsic::isVPCast, which reads VPIntrinsics.def — a \
         .def file llvmkit does not vendor",
    ),
    (
        "isVectorIntrinsicWithScalarOpAtArg",
        "needs VPIntrinsic::getVectorLengthParamPos, which reads \
         VPIntrinsics.def — a .def file llvmkit does not vendor",
    ),
    (
        "processShuffleMasks",
        "[permanent] splits a mask across physical registers; every caller is \
         in lib/CodeGen/SelectionDAG or lib/Target/{RISCV,X86}",
    ),
    ("propagateMetadata", "needs metadata modeling"),
    ("uniteAccessGroups", "needs metadata modeling"),
];

/// Every entry in [`MODELED`] resolves to a real public item.
///
/// Naming each as a value is the whole claim: the compiler must resolve the
/// path and instantiate the signature, so a rename or removal stops this file
/// compiling. No upstream counterpart — see the module docs.
#[test]
fn exercises_every_modeled_entry_point() {
    use llvmkit_ir::{
        DynBrand, create_interleave_mask, create_replicated_mask, create_sequential_mask,
        create_stride_mask, create_unary_mask, deinterleave_intrinsic_factor, find_scalar_element,
        horizontal_demanded_elements_for_first_operand, interleave_intrinsic_factor,
        is_splat_value, is_trivially_scalarizable, is_trivially_vectorizable,
        is_vector_intrinsic_with_struct_return_overload_at_field,
        mask_contains_all_one_or_undefined, mask_is_all_one_or_undefined,
        mask_is_all_zero_or_undefined, masked_slide_pair, narrow_shuffle_mask_elements,
        possibly_demanded_elements_in_mask, scale_shuffle_mask_elements, shuffle_demanded_elements,
        shuffle_mask_with_widest_elements, splat_index, splat_value, widen_shuffle_mask_elements,
        widen_shuffle_mask_elements_in_pairs,
    };

    // Mask-only functions: no brand to name.
    let _splat_index = splat_index;
    let _narrow = narrow_shuffle_mask_elements;
    let _widen = widen_shuffle_mask_elements;
    let _widen_in_pairs = widen_shuffle_mask_elements_in_pairs;
    let _scale = scale_shuffle_mask_elements;
    let _widest = shuffle_mask_with_widest_elements;
    let _demanded = shuffle_demanded_elements;
    let _horizontal = horizontal_demanded_elements_for_first_operand;
    let _slide_pair = masked_slide_pair;
    let _replicated = create_replicated_mask;
    let _interleave = create_interleave_mask;
    let _stride = create_stride_mask;
    let _sequential = create_sequential_mask;
    let _unary = create_unary_mask;

    // Value-taking functions: instantiated at a concrete brand.
    let _get_splat_value = splat_value::<DynBrand>;
    let _is_splat_value = is_splat_value::<DynBrand>;
    let _find_scalar_element = find_scalar_element::<DynBrand>;
    let _all_zero = mask_is_all_zero_or_undefined::<DynBrand>;
    let _all_one = mask_is_all_one_or_undefined::<DynBrand>;
    let _contains_one = mask_contains_all_one_or_undefined::<DynBrand>;
    let _possibly_demanded = possibly_demanded_elements_in_mask::<DynBrand>;

    // Intrinsic classifiers: no brand, and no target — see the GAPS note.
    let _vectorizable = is_trivially_vectorizable;
    let _scalarizable = is_trivially_scalarizable;
    let _struct_return = is_vector_intrinsic_with_struct_return_overload_at_field;
    let _interleave_factor = interleave_intrinsic_factor;
    let _deinterleave_factor = deinterleave_intrinsic_factor;
}

/// Both tables are sorted, duplicate-free and disjoint, and every gap carries
/// a reason.
///
/// A ledger nobody can read is a ledger nobody checks, so these are the
/// properties that keep it readable rather than merely present.
#[test]
fn the_tables_stay_readable() {
    let modeled: Vec<&str> = MODELED.iter().map(|(upstream, _)| *upstream).collect();
    let gaps: Vec<&str> = GAPS.iter().map(|(upstream, _)| *upstream).collect();

    let mut sorted_modeled = modeled.clone();
    sorted_modeled.sort_unstable();
    assert_eq!(modeled, sorted_modeled, "MODELED must stay sorted");

    let mut sorted_gaps = gaps.clone();
    sorted_gaps.sort_unstable();
    assert_eq!(gaps, sorted_gaps, "GAPS must stay sorted");

    let modeled_set: BTreeSet<&str> = modeled.iter().copied().collect();
    assert_eq!(modeled_set.len(), modeled.len(), "MODELED has a duplicate");

    let gap_set: BTreeSet<&str> = gaps.iter().copied().collect();
    assert_eq!(gap_set.len(), gaps.len(), "GAPS has a duplicate");

    let both: Vec<&&str> = modeled_set.intersection(&gap_set).collect();
    assert!(
        both.is_empty(),
        "a function cannot be both modeled and a gap: {both:?}"
    );

    for (name, reason) in GAPS {
        assert!(!reason.is_empty(), "{name} is recorded with no reason");
    }
    for (upstream, llvmkit) in MODELED {
        assert!(!llvmkit.is_empty(), "{upstream} is recorded with no name");
    }
}

/// The two tables together account for every upstream entry point.
///
/// This is the claim `vector_utils`' module docs make out loud — "everything
/// llvmkit's scope admits" — so it is the one worth failing on. Adding a
/// function to `vector_utils.rs` without moving its row out of [`GAPS`] breaks
/// this, and so does reconciling against a newer LLVM without updating
/// [`UPSTREAM_ENTRY_POINT_COUNT`] and [`DERIVED_FROM_LLVM`] together.
#[test]
fn the_tables_account_for_every_upstream_entry_point() {
    assert_eq!(
        MODELED.len() + GAPS.len(),
        UPSTREAM_ENTRY_POINT_COUNT,
        "VectorUtils.cpp defines {UPSTREAM_ENTRY_POINT_COUNT} functions as of \
         LLVM {DERIVED_FROM_LLVM}; the tables hold {} modeled + {} gaps",
        MODELED.len(),
        GAPS.len(),
    );
}
