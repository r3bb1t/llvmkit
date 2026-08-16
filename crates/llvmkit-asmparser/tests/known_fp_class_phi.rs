//! `computeKnownFPClass`'s `phi` arm.
//!
//! Every case comes from `class ComputeKnownFPClassTest` in
//! `llvm/unittests/Analysis/ValueTrackingTest.cpp`, IR inlined verbatim.
//! Upstream's `expectKnownFPClass(Mask, SignBit)` compares *both* the class
//! mask and the sign bit, so both are asserted here — three of these cases
//! exist precisely to pin the sign bit, which the class mask alone would not
//! catch.
//!
//! The self-reference cases are the ones worth having: a phi that names itself
//! must contribute nothing rather than recurse, and it must do so whichever
//! operand position it appears in.

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

/// Upstream's `expectKnownFPClass(Mask, SignBit)`, which always names `%A`.
fn expect(source: &str, classes: FpClassTest, sign_bit: Option<bool>) {
    let module = parse(source);
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::<DynBrand>::new(&data_layout);
    let known = compute_known_fp_class_all(named(&module, "A"), &query);
    assert_eq!(known.classes(), classes, "classes\n{source}");
    assert_eq!(known.sign_bit(), sign_bit, "sign bit\n{source}");
}

/// Ports `TEST_F(ComputeKnownFPClassTest, Phi)`: the union of the incomings.
#[test]
fn phi() {
    expect(
        r"
define float @test(i1 %cond, float nofpclass(nan inf) %arg0, float nofpclass(nan) %arg1) {
entry:
  br i1 %cond, label %bb0, label %bb1
bb0:
  br label %ret
bb1:
  br label %ret
ret:
  %A = phi float [ %arg0, %bb0 ],  [ %arg1, %bb1 ]
  ret float %A
}
",
        FpClassTest::ALL.difference(FpClassTest::NAN),
        None,
    );
}

/// Ports `TEST_F(ComputeKnownFPClassTest, PhiKnownSignFalse)`: both incomings
/// are `fabs` results, so the sign bit is known clear.
#[test]
fn phi_known_sign_false() {
    expect(
        r"
declare float @llvm.fabs.f32(float)
define float @test(i1 %cond, float nofpclass(nan) %arg0, float nofpclass(nan) %arg1) {
entry:
  br i1 %cond, label %bb0, label %bb1
bb0:
  %fabs.arg0 = call float @llvm.fabs.f32(float %arg0)
  br label %ret
bb1:
  %fabs.arg1 = call float @llvm.fabs.f32(float %arg1)
  br label %ret
ret:
  %A = phi float [ %fabs.arg0, %bb0 ],  [ %fabs.arg1, %bb1 ]
  ret float %A
}
",
        FpClassTest::POSITIVE,
        Some(false),
    );
}

/// Ports `TEST_F(ComputeKnownFPClassTest, PhiKnownSignTrue)`: both incomings
/// are negated `fabs` results, so the sign bit is known set.
#[test]
fn phi_known_sign_true() {
    expect(
        r"
declare float @llvm.fabs.f32(float)
define float @test(i1 %cond, float nofpclass(nan) %arg0, float %arg1) {
entry:
  br i1 %cond, label %bb0, label %bb1
bb0:
  %fabs.arg0 = call float @llvm.fabs.f32(float %arg0)
  %fneg.fabs.arg0 = fneg float %fabs.arg0
  br label %ret
bb1:
  %fabs.arg1 = call float @llvm.fabs.f32(float %arg1)
  %fneg.fabs.arg1 = fneg float %fabs.arg1
  br label %ret
ret:
  %A = phi float [ %fneg.fabs.arg0, %bb0 ],  [ %fneg.fabs.arg1, %bb1 ]
  ret float %A
}
",
        FpClassTest::NEGATIVE | FpClassTest::NAN,
        Some(true),
    );
}

/// Ports `TEST_F(ComputeKnownFPClassTest, SelfPhiFirstArg)` and
/// `SelfPhiSecondArg`: a phi naming itself contributes nothing, whichever
/// operand position it holds, so both answer from the other incoming alone.
#[test]
fn a_self_reference_contributes_nothing_from_either_position() {
    for source in [
        r"
define float @test(i1 %cond, float nofpclass(inf) %arg) {
entry:
  br i1 %cond, label %loop, label %ret
loop:
  %A = phi float [ %arg, %entry ], [ %A, %loop ]
  br label %loop
ret:
  ret float %A
}
",
        r"
define float @test(i1 %cond, float nofpclass(inf) %arg) {
entry:
  br i1 %cond, label %loop, label %ret
loop:
  %A = phi float [ %A, %loop ], [ %arg, %entry ]
  br label %loop
ret:
  ret float %A
}
",
    ] {
        expect(
            source,
            FpClassTest::ALL.difference(FpClassTest::INFINITY),
            None,
        );
    }
}

/// Ports `TEST_F(ComputeKnownFPClassTest, SelfPhiOnly)`: every incoming is a
/// self reference, so nothing is learned.
#[test]
fn a_phi_of_only_itself_answers_unknown() {
    expect(
        r"
define float @test(float %arg) {
entry:
  ret float 0.0
loop:
  %A = phi float [ %A, %loop ]
  br label %loop
}
",
        FpClassTest::ALL,
        None,
    );
}
