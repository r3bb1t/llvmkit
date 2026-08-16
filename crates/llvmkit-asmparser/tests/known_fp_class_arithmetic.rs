//! `computeKnownFPClass`'s arithmetic arms — `fadd`, `fsub`, `fmul`.
//!
//! Every case comes from `class ComputeKnownFPClassTest` in
//! `llvm/unittests/Analysis/ValueTrackingTest.cpp`, IR inlined verbatim. That
//! harness runs `computeKnownFPClass(V, DL)` on each named instruction and
//! compares the resulting `KnownFPClasses` mask and `SignBit` against
//! `expectKnownFPClass(Mask, SignBit, V)`, which is what the tables below
//! carry.
//!
//! Upstream builds every operand out of `nofpclass` parameter attributes,
//! because that is the only way to hand these arms an operand whose class is
//! known without constant-folding the expression away. That attribute is
//! modeled, which is what makes these fixtures portable at all.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    DynBrand, FpClassTest, Module, Unverified, Value, ValueTrackingQuery,
    compute_known_fp_class_all,
};

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}\n--- source ---\n{source}"),
    }
}

fn named<'m>(module: &'m Module<DynBrand, Unverified>, name: &str) -> Value<'m, DynBrand> {
    module
        .as_view()
        .functions()
        .flat_map(|function| function.basic_blocks())
        .flat_map(|block| block.instructions())
        .find(|instruction| instruction.name().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("fixture defines %{name}"))
        .to_erased()
}

/// Upstream's `expectKnownFPClass(Mask, SignBit, V)`, for each named value.
fn expect_classes(source: &str, cases: &[(&str, FpClassTest)]) {
    let module = parse(source);
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::<DynBrand>::new(&data_layout);
    for (name, expected) in cases {
        let known = compute_known_fp_class_all(named(&module, name), &query);
        assert_eq!(
            known.classes(),
            *expected,
            "computeKnownFPClass(%{name}) classes"
        );
    }
}

/// Ports `TEST_F(ComputeKnownFPClassTest, FAdd)`.
///
/// `%A5` is `fadd %nnan, %nnan` — the self-add path — and upstream still
/// expects `fcAllFlags`, because two non-NaN operands can still add to
/// infinity minus infinity.
#[test]
fn fadd() {
    expect_classes(
        r"
define float @test(float nofpclass(nan inf) %nnan.ninf, float nofpclass(nan) %nnan, float nofpclass(qnan) %no.qnan, float %unknown) {
  %A = fadd float %nnan, %nnan.ninf
  %A2 = fadd float %nnan.ninf, %nnan
  %A3 = fadd float %nnan.ninf, %unknown
  %A4 = fadd float %nnan.ninf, %no.qnan
  %A5 = fadd float %nnan, %nnan
  ret float %A
}
",
        &[
            ("A", FpClassTest::FINITE | FpClassTest::INFINITY),
            ("A2", FpClassTest::FINITE | FpClassTest::INFINITY),
            ("A3", FpClassTest::ALL),
            ("A4", FpClassTest::ALL),
            ("A5", FpClassTest::ALL),
        ],
    );
}

/// Ports `TEST_F(ComputeKnownFPClassTest, FSub)`.
#[test]
fn fsub() {
    expect_classes(
        r"
define float @test(float nofpclass(nan inf) %nnan.ninf, float nofpclass(nan) %nnan, float nofpclass(qnan) %no.qnan, float %unknown) {
  %A = fsub float %nnan, %nnan.ninf
  %A2 = fsub float %nnan.ninf, %nnan
  %A3 = fsub float %nnan.ninf, %unknown
  %A4 = fsub float %nnan.ninf, %no.qnan
  %A5 = fsub float %nnan, %nnan
  ret float %A
}
",
        &[
            ("A", FpClassTest::FINITE | FpClassTest::INFINITY),
            ("A2", FpClassTest::FINITE | FpClassTest::INFINITY),
            ("A3", FpClassTest::ALL),
            ("A4", FpClassTest::ALL),
            ("A5", FpClassTest::ALL),
        ],
    );
}

/// Ports `TEST_F(ComputeKnownFPClassTest, FMul)`.
///
/// `%A5` is `fmul %nnan, %nnan` — the `x * x` square path — which is why it
/// alone answers `fcPositive | fcNan` rather than `fcAllFlags`.
#[test]
fn fmul() {
    expect_classes(
        r"
define float @test(float noundef nofpclass(nan inf) %nnan.ninf0, float noundef nofpclass(nan inf) %nnan.ninf1, float noundef nofpclass(nan) %nnan, float noundef nofpclass(qnan) %no.qnan, float noundef %unknown) {
  %A = fmul float %nnan.ninf0, %nnan.ninf1
  %A2 = fmul float %nnan.ninf0, %nnan
  %A3 = fmul float %nnan, %nnan.ninf0
  %A4 = fmul float %nnan.ninf0, %no.qnan
  %A5 = fmul float %nnan, %nnan
  ret float %A
}
",
        &[
            ("A", FpClassTest::FINITE | FpClassTest::INFINITY),
            ("A2", FpClassTest::ALL),
            ("A3", FpClassTest::ALL),
            ("A4", FpClassTest::ALL),
            ("A5", FpClassTest::POSITIVE | FpClassTest::NAN),
        ],
    );
}

/// Ports `TEST_F(ComputeKnownFPClassTest, FMulNoZero)`.
///
/// The zero-aware half of the `fmul` arm: `KnownFPClass::fmul` needs both a
/// no-zero and a no-infinity fact before it can rule NaN out, which is why
/// most of these still answer `fcAllFlags`.
#[test]
fn fmul_no_zero() {
    expect_classes(
        r"
define float @test(float noundef nofpclass(zero) %no.zero, float noundef nofpclass(zero nan) %no.zero.nan0, float noundef nofpclass(zero nan) %no.zero.nan1, float noundef nofpclass(nzero nan) %no.negzero.nan, float noundef nofpclass(pzero nan) %no.poszero.nan, float noundef nofpclass(inf nan) %no.inf.nan, float noundef nofpclass(inf) %no.inf, float noundef nofpclass(nan) %no.nan) {
  %A = fmul float %no.zero.nan0, %no.zero.nan1
  %A2 = fmul float %no.zero, %no.zero
  %A3 = fmul float %no.poszero.nan, %no.zero.nan0
  %A4 = fmul float %no.nan, %no.zero
  %A5 = fmul float %no.zero, %no.inf
  %A6 = fmul float %no.zero.nan0, %no.nan
  %A7 = fmul float %no.nan, %no.zero.nan0
  ret float %A
}
",
        &[
            ("A", FpClassTest::FINITE | FpClassTest::INFINITY),
            ("A2", FpClassTest::POSITIVE | FpClassTest::NAN),
            ("A3", FpClassTest::ALL),
            ("A4", FpClassTest::ALL),
            ("A5", FpClassTest::ALL),
            ("A6", FpClassTest::ALL),
            ("A7", FpClassTest::ALL),
        ],
    );
}
