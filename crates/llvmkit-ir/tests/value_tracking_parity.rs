const UPSTREAM_PARITY_ANCHORS: &[&str] = &[
    "llvm/include/llvm/Support/KnownBits.h",
    "llvm/lib/Support/KnownBits.cpp",
    "llvm/include/llvm/Analysis/ValueTracking.h",
    "llvm/lib/Analysis/ValueTracking.cpp",
    "llvm/include/llvm/Analysis/DemandedBits.h",
    "llvm/lib/Analysis/DemandedBits.cpp",
    "llvm/lib/Transforms/InstCombine/InstCombineSimplifyDemanded.cpp",
];

/// Parity ledger for `llvm/include/llvm/Support/KnownBits.h`,
/// `llvm/lib/Support/KnownBits.cpp`,
/// `llvm/include/llvm/Analysis/ValueTracking.h`,
/// `llvm/lib/Analysis/ValueTracking.cpp`,
/// `llvm/include/llvm/Analysis/DemandedBits.h`,
/// `llvm/lib/Analysis/DemandedBits.cpp`, and
/// `llvm/lib/Transforms/InstCombine/InstCombineSimplifyDemanded.cpp`.
#[test]
fn parity_ledger_mentions_every_upstream_anchor() {
    const REQUIRED: &[&str] = &[
        "llvm/include/llvm/Support/KnownBits.h",
        "llvm/lib/Support/KnownBits.cpp",
        "llvm/include/llvm/Analysis/ValueTracking.h",
        "llvm/lib/Analysis/ValueTracking.cpp",
        "llvm/include/llvm/Analysis/DemandedBits.h",
        "llvm/lib/Analysis/DemandedBits.cpp",
        "llvm/lib/Transforms/InstCombine/InstCombineSimplifyDemanded.cpp",
    ];

    for anchor in REQUIRED {
        assert!(
            UPSTREAM_PARITY_ANCHORS.contains(anchor),
            "missing KnownBits / ValueTracking parity anchor: {anchor}"
        );
    }
}
