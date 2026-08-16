//! Port of `llvm/test/Analysis/ValueTracking/recurrence-knownbits.ll`.
//!
//! The fixture is the upstream file, byte for byte. Every function in it has
//! the shape
//!
//! ```text
//! entry:
//!   br label %loop
//! loop:
//!   %iv = phi i64 [<start>, %entry], [%iv.next, %loop]
//!   %iv.next = <binop> ...
//!   br i1 %c, label %exit, label %loop
//! exit:
//!   %res = <and|or> i64 %iv, <mask>
//!   ret i64 %res
//! ```
//!
//! and its CHECK line records what `-passes=instcombine` folds `%res` to.
//! llvmkit has no runnable pass pipeline (see `CLAUDE.md`), but that fold is a
//! known-bits question: InstCombine replaces `%res` with a constant exactly
//! when `%res` has no unknown bits left. So the oracle here is the CHECK line
//! verbatim — `compute_known_bits` of the fixture's own `%res`, compared
//! against the constant upstream prints, with `None` standing for the
//! functions whose CHECK keeps `%res` as an instruction.
//!
//! Twelve of the fifteen functions reproduce their CHECK exactly. The other
//! three are the ones whose fold does not come from the known-bits PHI arm at
//! all; they are listed in `UPSTREAM_GAPS` with the reason, and asserted to
//! stay unfolded so the gap cannot close silently.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    DynBrand, Module, Signedness, Unverified, ValueTrackingQuery, compute_known_bits,
};

const FIXTURE: &str =
    include_str!("fixtures/upstream/Analysis/ValueTracking/recurrence-knownbits.ll");

/// `(function, what upstream's CHECK folds `%res` to)`. `None` means the CHECK
/// keeps `%res` as an instruction — upstream proved nothing about it.
///
/// Read straight off the `; CHECK-NEXT: ret i64 …` line of each function.
const UPSTREAM_CHECKS: &[(&str, Option<&str>)] = &[
    ("test_lshr", Some("1023")),
    ("test_add", Some("0")),
    ("test_sub", Some("0")),
    ("test_udiv", Some("0")),
    ("test_udiv_neg", None),
    ("test_urem", Some("0")),
    ("test_ashr_zeros", Some("1023")),
    ("test_ashr_ones", Some("-1")),
    ("test_ashr_ones2", Some("-1")),
    ("test_ashr_unknown", None),
    ("test_ashr_wrong_op", None),
    ("test_shl", Some("0")),
];

/// The three functions whose CHECK llvmkit does **not** reproduce, each with
/// the upstream constant it expects and why known bits alone cannot get there.
///
/// Both reasons are InstCombine work this crate does not perform, so the
/// entries assert `%res` stays *unfolded* rather than asserting a transform
/// llvmkit has not implemented. If a later change makes one of these fold, the
/// test fails and the row has to move into `UPSTREAM_CHECKS`.
const UPSTREAM_GAPS: &[(&str, &str, &str)] = &[
    (
        "test_mul",
        "0",
        "needs bit 1 of %iv known zero; the mul arm keeps only \
         min(countMinTrailingZeros(8), countMinTrailingZeros(2)) = 1 trailing zero. \
         Upstream first canonicalizes `mul i64 %iv, 2` to `shl i64 %iv, 1`, after which \
         the shift arm keeps all three trailing zeros of the start value — which is why \
         @test_shl, the same recurrence already canonicalized, does reach its CHECK",
    ),
    (
        "test_and",
        "2047",
        "needs bits 11..63 of %iv known zero *and* bit 10 known one. The and/or arm only \
         ever sets low zero bits, and min(countMinTrailingZeros(1025), \
         countMinTrailingZeros(1024)) is 0, so it contributes nothing; the fallthrough \
         intersection leaves bit 10 unknown. Upstream reaches 2047 by simplifying the \
         loop away, not by known bits",
    ),
    (
        "test_or",
        "2047",
        "as @test_and. The intersection does prove bit 10 is one here, but bits 11..63 \
         stay unknown, so `or %iv, 1023` is still not a constant",
    ),
];

fn parsed() -> Module<DynBrand, Unverified> {
    parser::parse_dynamic(FIXTURE).expect("upstream recurrence fixture parses")
}

/// `%res` of `@function`, folded as far as known bits allow, rendered the way
/// upstream's CHECK renders it.
fn folded_result(module: &Module<DynBrand, Unverified>, function: &str) -> Option<String> {
    let f = module
        .as_view()
        .functions()
        .find(|f| f.name() == function)
        .unwrap_or_else(|| panic!("fixture defines @{function}"));
    let result = f
        .basic_blocks()
        .flat_map(|block| block.instructions())
        .find(|instruction| instruction.name().as_deref() == Some("res"))
        .unwrap_or_else(|| panic!("@{function} defines %res"))
        .to_erased();

    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::new(&data_layout);
    compute_known_bits(result, &query)
        .expect("known bits of %res")
        .constant()
        .map(|value| value.to_string_radix(10, Signedness::Signed))
}

/// Every function whose `; CHECK-NEXT: ret i64 …` line llvmkit reproduces.
///
/// This exercises the recurrence arm of `computeKnownBitsFromOperator`
/// (`ValueTracking.cpp`) end to end: the `shl` / `lshr` / `ashr` / `udiv` /
/// `urem` shift-and-divide cases, the `add` / `sub` common-trailing-zeros
/// case, both signs of the `ashr` sign-bit extension, the operand-order
/// independence of `matchSimpleRecurrence` (`@test_ashr_ones2`), and the
/// negative cases where the phi is the wrong operand (`@test_udiv_neg`,
/// `@test_ashr_wrong_op`) or the start value is unknown
/// (`@test_ashr_unknown`).
#[test]
fn recurrence_fixture_reproduces_its_upstream_checks() {
    let module = parsed();
    for (function, expected) in UPSTREAM_CHECKS {
        assert_eq!(
            folded_result(&module, function).as_deref(),
            *expected,
            "@{function}"
        );
    }
}

/// The three fixture functions whose CHECK is out of reach, pinned as gaps.
///
/// See `UPSTREAM_GAPS` for the per-function reason.
#[test]
fn recurrence_fixture_gaps_stay_unfolded() {
    let module = parsed();
    for (function, upstream, reason) in UPSTREAM_GAPS {
        assert_eq!(
            folded_result(&module, function),
            None,
            "@{function} unexpectedly folded; upstream gets {upstream} because it {reason}"
        );
    }
}

/// Every function in the fixture is accounted for by exactly one of the two
/// tables, so a future upstream sync that adds a case cannot pass unnoticed.
#[test]
fn every_fixture_function_is_covered() {
    let module = parsed();
    let mut uncovered: Vec<String> = Vec::new();
    for f in module.as_view().functions() {
        let name = f.name().to_string();
        let checked = UPSTREAM_CHECKS.iter().any(|(known, _)| *known == name);
        let gap = UPSTREAM_GAPS.iter().any(|(known, _, _)| *known == name);
        assert!(!(checked && gap), "@{name} is in both tables");
        if !checked && !gap {
            uncovered.push(name);
        }
    }
    assert!(
        uncovered.is_empty(),
        "unclassified fixture cases: {uncovered:?}"
    );
    assert_eq!(
        UPSTREAM_CHECKS.len() + UPSTREAM_GAPS.len(),
        module.as_view().functions().count(),
        "table size drifted from the fixture"
    );
}
