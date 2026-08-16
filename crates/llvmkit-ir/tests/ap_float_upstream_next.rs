//! Port of `TEST(APFloatTest, next)` from
//! `llvm/unittests/ADT/APFloatTest.cpp` in the vendored `llvmorg-22.1.4` tree.
//!
//! Upstream walks nextUp/nextDown over every special value IEEE-754R 2008
//! prescribes, then over the boundaries between binade, denormal, and zero.
//! Each `test = …; EXPECT_EQ(test.next(…), …); EXPECT_TRUE(…)` block becomes
//! one row here, keeping upstream's operands, direction, expected status, and
//! assertion.
//!
//! `next(false)` is nextUp and `next(true)` is nextDown, which is why the
//! rows below read `TowardPositive` / `TowardNegative`.
//!
//! Generated from the C++ test; regenerate rather than hand-edit.

use llvmkit_ir::{
    ApFloat, ApFloatNextDirection, ApFloatSemantics, ApFloatSign, ApFloatStatus as S, NanPayload,
    RoundingMode,
};

const QUAD: ApFloatSemantics = ApFloatSemantics::IeeeQuad;

fn parse(semantics: ApFloatSemantics, text: &str) -> ApFloat {
    ApFloat::from_string(semantics, text, RoundingMode::NearestTiesToEven)
        .unwrap_or_else(|e| panic!("upstream spells {text:?}, which must parse: {e}"))
        .0
}

/// Port of `TEST(APFloatTest, next)`.
#[test]
fn upstream_next() {
    let rows: Vec<(ApFloat, bool, S, ApFloat)> = vec![
        (
            ApFloat::inf(QUAD, ApFloatSign::Positive),
            false,
            S::OK,
            ApFloat::inf(QUAD, ApFloatSign::Positive),
        ),
        (
            ApFloat::inf(QUAD, ApFloatSign::Positive),
            true,
            S::OK,
            ApFloat::largest(QUAD, ApFloatSign::Positive),
        ),
        (
            ApFloat::inf(QUAD, ApFloatSign::Negative),
            false,
            S::OK,
            ApFloat::largest(QUAD, ApFloatSign::Negative),
        ),
        (
            ApFloat::inf(QUAD, ApFloatSign::Negative),
            true,
            S::OK,
            ApFloat::inf(QUAD, ApFloatSign::Negative),
        ),
        (
            ApFloat::largest(QUAD, ApFloatSign::Positive),
            false,
            S::OK,
            ApFloat::inf(QUAD, ApFloatSign::Positive),
        ),
        (
            ApFloat::largest(QUAD, ApFloatSign::Positive),
            true,
            S::OK,
            parse(QUAD, "0x1.fffffffffffffffffffffffffffep+16383"),
        ),
        (
            ApFloat::largest(QUAD, ApFloatSign::Negative),
            false,
            S::OK,
            parse(QUAD, "-0x1.fffffffffffffffffffffffffffep+16383"),
        ),
        (
            ApFloat::largest(QUAD, ApFloatSign::Negative),
            true,
            S::OK,
            ApFloat::inf(QUAD, ApFloatSign::Negative),
        ),
        (
            parse(QUAD, "0x0.0000000000000000000000000001p-16382"),
            false,
            S::OK,
            parse(QUAD, "0x0.0000000000000000000000000002p-16382"),
        ),
        (
            parse(QUAD, "0x0.0000000000000000000000000001p-16382"),
            true,
            S::OK,
            ApFloat::zero(QUAD, ApFloatSign::Positive),
        ),
        (
            parse(QUAD, "-0x0.0000000000000000000000000001p-16382"),
            false,
            S::OK,
            ApFloat::zero(QUAD, ApFloatSign::Negative),
        ),
        (
            parse(QUAD, "-0x0.0000000000000000000000000001p-16382"),
            true,
            S::OK,
            parse(QUAD, "-0x0.0000000000000000000000000002p-16382"),
        ),
        (
            ApFloat::qnan(QUAD, ApFloatSign::Positive, NanPayload::Absent),
            false,
            S::OK,
            ApFloat::qnan(QUAD, ApFloatSign::Positive, NanPayload::Absent),
        ),
        (
            ApFloat::qnan(QUAD, ApFloatSign::Positive, NanPayload::Absent),
            true,
            S::OK,
            ApFloat::qnan(QUAD, ApFloatSign::Positive, NanPayload::Absent),
        ),
        (
            ApFloat::snan(QUAD, ApFloatSign::Positive, NanPayload::Absent),
            false,
            S::INVALID_OP,
            ApFloat::qnan(QUAD, ApFloatSign::Positive, NanPayload::Absent),
        ),
        (
            ApFloat::snan(QUAD, ApFloatSign::Positive, NanPayload::Absent),
            true,
            S::INVALID_OP,
            ApFloat::qnan(QUAD, ApFloatSign::Positive, NanPayload::Absent),
        ),
        (
            ApFloat::zero(QUAD, ApFloatSign::Positive),
            false,
            S::OK,
            ApFloat::smallest(QUAD, ApFloatSign::Positive),
        ),
        (
            ApFloat::zero(QUAD, ApFloatSign::Positive),
            true,
            S::OK,
            ApFloat::smallest(QUAD, ApFloatSign::Negative),
        ),
        (
            ApFloat::zero(QUAD, ApFloatSign::Negative),
            false,
            S::OK,
            ApFloat::smallest(QUAD, ApFloatSign::Positive),
        ),
        (
            ApFloat::zero(QUAD, ApFloatSign::Negative),
            true,
            S::OK,
            ApFloat::smallest(QUAD, ApFloatSign::Negative),
        ),
        (
            parse(QUAD, "0x0.ffffffffffffffffffffffffffffp-16382"),
            false,
            S::OK,
            parse(QUAD, "0x1.0000000000000000000000000000p-16382"),
        ),
        (
            parse(QUAD, "-0x0.ffffffffffffffffffffffffffffp-16382"),
            true,
            S::OK,
            parse(QUAD, "-0x1.0000000000000000000000000000p-16382"),
        ),
        (
            parse(QUAD, "-0x1.0000000000000000000000000000p-16382"),
            false,
            S::OK,
            parse(QUAD, "-0x0.ffffffffffffffffffffffffffffp-16382"),
        ),
        (
            parse(QUAD, "+0x1.0000000000000000000000000000p-16382"),
            true,
            S::OK,
            parse(QUAD, "+0x0.ffffffffffffffffffffffffffffp-16382"),
        ),
        (
            parse(QUAD, "-0x1p+1"),
            false,
            S::OK,
            parse(QUAD, "-0x1.ffffffffffffffffffffffffffffp+0"),
        ),
        (
            parse(QUAD, "0x1p+1"),
            true,
            S::OK,
            parse(QUAD, "0x1.ffffffffffffffffffffffffffffp+0"),
        ),
        (
            parse(QUAD, "0x1.ffffffffffffffffffffffffffffp+0"),
            false,
            S::OK,
            parse(QUAD, "0x1p+1"),
        ),
        (
            parse(QUAD, "-0x1.ffffffffffffffffffffffffffffp+0"),
            true,
            S::OK,
            parse(QUAD, "-0x1p+1"),
        ),
        (
            parse(QUAD, "-0x0.ffffffffffffffffffffffffffffp-16382"),
            false,
            S::OK,
            parse(QUAD, "-0x0.fffffffffffffffffffffffffffep-16382"),
        ),
        (
            parse(QUAD, "0x0.ffffffffffffffffffffffffffffp-16382"),
            true,
            S::OK,
            parse(QUAD, "0x0.fffffffffffffffffffffffffffep-16382"),
        ),
        (
            parse(QUAD, "0x1.0000000000000000000000000000p-16382"),
            false,
            S::OK,
            parse(QUAD, "0x1.0000000000000000000000000001p-16382"),
        ),
        (
            parse(QUAD, "-0x1.0000000000000000000000000000p-16382"),
            true,
            S::OK,
            parse(QUAD, "-0x1.0000000000000000000000000001p-16382"),
        ),
        (
            parse(QUAD, "-0x1p-16381"),
            false,
            S::OK,
            parse(QUAD, "-0x1.ffffffffffffffffffffffffffffp-16382"),
        ),
        (
            parse(QUAD, "-0x1.ffffffffffffffffffffffffffffp-16382"),
            true,
            S::OK,
            parse(QUAD, "-0x1p-16381"),
        ),
        (
            parse(QUAD, "0x1.ffffffffffffffffffffffffffffp-16382"),
            false,
            S::OK,
            parse(QUAD, "0x1p-16381"),
        ),
        (
            parse(QUAD, "0x1p-16381"),
            true,
            S::OK,
            parse(QUAD, "0x1.ffffffffffffffffffffffffffffp-16382"),
        ),
        (
            parse(QUAD, "0x0.ffffffffffffffffffffffff000cp-16382"),
            false,
            S::OK,
            parse(QUAD, "0x0.ffffffffffffffffffffffff000dp-16382"),
        ),
        (
            parse(QUAD, "0x0.ffffffffffffffffffffffff000cp-16382"),
            true,
            S::OK,
            parse(QUAD, "0x0.ffffffffffffffffffffffff000bp-16382"),
        ),
        (
            parse(QUAD, "-0x0.ffffffffffffffffffffffff000cp-16382"),
            false,
            S::OK,
            parse(QUAD, "-0x0.ffffffffffffffffffffffff000bp-16382"),
        ),
        (
            parse(QUAD, "-0x0.ffffffffffffffffffffffff000cp-16382"),
            true,
            S::OK,
            parse(QUAD, "-0x0.ffffffffffffffffffffffff000dp-16382"),
        ),
        (
            parse(QUAD, "0x1.ffffffffffffffffffffffff000cp-16000"),
            false,
            S::OK,
            parse(QUAD, "0x1.ffffffffffffffffffffffff000dp-16000"),
        ),
        (
            parse(QUAD, "0x1.ffffffffffffffffffffffff000cp-16000"),
            true,
            S::OK,
            parse(QUAD, "0x1.ffffffffffffffffffffffff000bp-16000"),
        ),
        (
            parse(QUAD, "-0x1.ffffffffffffffffffffffff000cp-16000"),
            false,
            S::OK,
            parse(QUAD, "-0x1.ffffffffffffffffffffffff000bp-16000"),
        ),
        (
            parse(QUAD, "-0x1.ffffffffffffffffffffffff000cp-16000"),
            true,
            S::OK,
            parse(QUAD, "-0x1.ffffffffffffffffffffffff000dp-16000"),
        ),
    ];
    let mut failures = Vec::new();
    for (index, (value, down, status, expected)) in rows.iter().enumerate() {
        let direction = if *down {
            ApFloatNextDirection::TowardNegative
        } else {
            ApFloatNextDirection::TowardPositive
        };
        let (got, got_status) = value.next(direction);
        if got_status != *status {
            failures.push(format!(
                "next[{index}]: expected status {status:?}, got {got_status:?}"
            ));
            continue;
        }
        if !expected.bitwise_is_equal(&got) {
            failures.push(format!("next[{index}]: expected {expected:?}, got {got:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} upstream next rows diverge:\n{}",
        failures.len(),
        rows.len(),
        failures.join("\n")
    );
}
