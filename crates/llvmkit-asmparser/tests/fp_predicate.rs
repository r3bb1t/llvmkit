//! Ports of `fcmpToClassTest` — what an `fcmp` proves about the class of its
//! operand.
//!
//! Source: `llvm/unittests/Analysis/ValueTrackingTest.cpp`, the four
//! `ComputeKnownFPClassTest.FCmpToClassTest_*` cases. Each is ported whole:
//! every comparison in the fixture, with upstream's own expected mask.
//!
//! Upstream calls `fcmpToClassTest(Pred, F, LHS, RHS)` and leaves
//! `LookThroughSrc` at its `true` default, so these pass `true`.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    Dyn, DynBrand, FloatPredicate, FpClassTest, FunctionValue, Module, Unverified, Value,
    fcmp_implies_class_of_class, fcmp_to_class_test,
};

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}\n--- source ---\n{source}"),
    }
}

/// The fixture's function definition — upstream's `*A->getFunction()`, which is
/// what supplies the denormal mode.
fn defined_function<'m>(
    module: &'m Module<DynBrand, Unverified>,
) -> FunctionValue<'m, Dyn, DynBrand> {
    let definition = module
        .as_view()
        .functions()
        .find(|function| function.basic_blocks().next().is_some())
        .expect("fixture defines a function")
        .id();
    module.view(definition)
}

/// The two operands of the `fcmp` named `%name`.
fn compare_operands<'m>(
    module: &'m Module<DynBrand, Unverified>,
    name: &str,
) -> (Value<'m, DynBrand>, Value<'m, DynBrand>) {
    let compare = module
        .as_view()
        .functions()
        .flat_map(|function| function.basic_blocks())
        .flat_map(|block| block.instructions())
        .find(|candidate| candidate.name().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("fixture defines %{name}"));
    let compare = compare
        .kind()
        .and_then(|kind| kind.as_cmp())
        .unwrap_or_else(|| panic!("%{name} is a compare"));
    (compare.lhs(), compare.rhs())
}

/// The sole argument of the fixture's function *definition*.
fn only_argument<'m>(module: &'m Module<DynBrand, Unverified>) -> Value<'m, DynBrand> {
    let definition = module
        .as_view()
        .functions()
        .find(|function| function.basic_blocks().next().is_some())
        .expect("fixture defines a function")
        .id();
    module
        .view(definition)
        .params()
        .next()
        .expect("fixture's function takes an argument")
        .into_erased()
}

/// Comparing against a NaN never depends on the value: ordered is always false,
/// unordered always true.
///
/// Ports `ComputeKnownFPClassTest.FCmpToClassTest_OrdNan`
/// (`llvm/unittests/Analysis/ValueTrackingTest.cpp`).
#[test]
fn fcmp_to_class_test_against_a_nan() {
    let module = parse(
        r"
define i1 @test(double %arg) {
  %A = fcmp ord double %arg, 0x7FF8000000000000
  %A2 = fcmp uno double %arg, 0x7FF8000000000000
  %A3 = fcmp oeq double %arg, 0x7FF8000000000000
  %A4 = fcmp ueq double %arg, 0x7FF8000000000000
  ret i1 %A
}
",
    );

    let function = defined_function(&module);
    let (lhs, rhs) = compare_operands(&module, "A");
    assert_eq!(
        fcmp_to_class_test(FloatPredicate::Ord, function, lhs, rhs, true),
        Some((lhs, FpClassTest::NONE))
    );

    let (lhs, rhs) = compare_operands(&module, "A2");
    assert_eq!(
        fcmp_to_class_test(FloatPredicate::Uno, function, lhs, rhs, true),
        Some((lhs, FpClassTest::ALL))
    );

    let (lhs, rhs) = compare_operands(&module, "A3");
    assert_eq!(
        fcmp_to_class_test(FloatPredicate::Oeq, function, lhs, rhs, true),
        Some((lhs, FpClassTest::NONE))
    );

    // Upstream reads `%A3`'s operands here too, not `%A4`'s; they are the same
    // pair, and the port keeps the same reading.
    assert_eq!(
        fcmp_to_class_test(FloatPredicate::Ueq, function, lhs, rhs, true),
        Some((lhs, FpClassTest::ALL))
    );
}

/// Comparing against negative infinity — the `IsInf` arm, with the value itself
/// on the left.
///
/// Ports `ComputeKnownFPClassTest.FCmpToClassTest_NInf`
/// (`llvm/unittests/Analysis/ValueTrackingTest.cpp`).
#[test]
fn fcmp_to_class_test_against_negative_infinity() {
    let module = parse(
        r"
define i1 @test(double %arg) {
  %A = fcmp olt double %arg, 0xFFF0000000000000
  %A2 = fcmp uge double %arg, 0xFFF0000000000000
  %A3 = fcmp ogt double %arg, 0xFFF0000000000000
  %A4 = fcmp ule double %arg, 0xFFF0000000000000
  %A5 = fcmp oge double %arg, 0xFFF0000000000000
  %A6 = fcmp ult double %arg, 0xFFF0000000000000
  ret i1 %A
}
",
    );

    // Nothing is ordered and less than negative infinity; everything is
    // unordered with or at least it.
    let function = defined_function(&module);
    let (lhs, rhs) = compare_operands(&module, "A");
    assert_eq!(
        fcmp_to_class_test(FloatPredicate::Olt, function, lhs, rhs, true),
        Some((lhs, FpClassTest::NONE))
    );

    let (lhs, rhs) = compare_operands(&module, "A2");
    assert_eq!(
        fcmp_to_class_test(FloatPredicate::Uge, function, lhs, rhs, true),
        Some((lhs, FpClassTest::ALL))
    );

    let (lhs, rhs) = compare_operands(&module, "A3");
    assert_eq!(
        fcmp_to_class_test(FloatPredicate::Ogt, function, lhs, rhs, true),
        Some((
            lhs,
            FpClassTest::NEGATIVE_INFINITY
                .union(FpClassTest::NAN)
                .complement()
        ))
    );

    let (lhs, rhs) = compare_operands(&module, "A4");
    assert_eq!(
        fcmp_to_class_test(FloatPredicate::Ule, function, lhs, rhs, true),
        Some((lhs, FpClassTest::NEGATIVE_INFINITY.union(FpClassTest::NAN)))
    );

    let (lhs, rhs) = compare_operands(&module, "A5");
    assert_eq!(
        fcmp_to_class_test(FloatPredicate::Oge, function, lhs, rhs, true),
        Some((lhs, FpClassTest::NAN.complement()))
    );

    let (lhs, rhs) = compare_operands(&module, "A6");
    assert_eq!(
        fcmp_to_class_test(FloatPredicate::Ult, function, lhs, rhs, true),
        Some((lhs, FpClassTest::NAN))
    );
}

/// The same comparisons through an `llvm.fabs`, which answers about the *call's
/// operand* rather than the call.
///
/// Ports `ComputeKnownFPClassTest.FCmpToClassTest_FabsNInf`
/// (`llvm/unittests/Analysis/ValueTrackingTest.cpp`).
#[test]
fn fcmp_to_class_test_against_negative_infinity_through_fabs() {
    let module = parse(
        r"
declare double @llvm.fabs.f64(double)

define i1 @test(double %arg) {
  %fabs.arg = call double @llvm.fabs.f64(double %arg)
  %A = fcmp olt double %fabs.arg, 0xFFF0000000000000
  %A2 = fcmp uge double %fabs.arg, 0xFFF0000000000000
  %A3 = fcmp ogt double %fabs.arg, 0xFFF0000000000000
  %A4 = fcmp ule double %fabs.arg, 0xFFF0000000000000
  %A5 = fcmp oge double %fabs.arg, 0xFFF0000000000000
  %A6 = fcmp ult double %fabs.arg, 0xFFF0000000000000
  ret i1 %A
}
",
    );
    // Every answer is about the argument, not the `fabs` call.
    let argument = only_argument(&module);

    let function = defined_function(&module);
    let (lhs, rhs) = compare_operands(&module, "A");
    assert_eq!(
        fcmp_to_class_test(FloatPredicate::Olt, function, lhs, rhs, true),
        Some((argument, FpClassTest::NONE))
    );

    let (lhs, rhs) = compare_operands(&module, "A2");
    assert_eq!(
        fcmp_to_class_test(FloatPredicate::Uge, function, lhs, rhs, true),
        Some((argument, FpClassTest::ALL))
    );

    // `fabs(x) > -inf` is exactly `x` being ordered — no negative infinity to
    // exclude, because `fabs` cannot produce one.
    let (lhs, rhs) = compare_operands(&module, "A3");
    assert_eq!(
        fcmp_to_class_test(FloatPredicate::Ogt, function, lhs, rhs, true),
        Some((argument, FpClassTest::NAN.complement()))
    );

    let (lhs, rhs) = compare_operands(&module, "A4");
    assert_eq!(
        fcmp_to_class_test(FloatPredicate::Ule, function, lhs, rhs, true),
        Some((argument, FpClassTest::NAN))
    );

    let (lhs, rhs) = compare_operands(&module, "A5");
    assert_eq!(
        fcmp_to_class_test(FloatPredicate::Oge, function, lhs, rhs, true),
        Some((argument, FpClassTest::NAN.complement()))
    );

    let (lhs, rhs) = compare_operands(&module, "A6");
    assert_eq!(
        fcmp_to_class_test(FloatPredicate::Ult, function, lhs, rhs, true),
        Some((argument, FpClassTest::NAN))
    );
}

/// The zero arm, reached through an `llvm.fabs`, for every predicate.
///
/// Ports `ComputeKnownFPClassTest.fcmpImpliesClass_fabs_zero`
/// (`llvm/unittests/Analysis/ValueTrackingTest.cpp`) — all fourteen assertions,
/// each reading upstream's `std::get<1>`, the classes if the comparison is true.
///
/// This arm only runs when the function's denormal mode has IEEE inputs, which
/// is what a function carrying no `denormal-fp-math` attribute has.
#[test]
fn fcmp_implies_class_through_fabs_against_zero() {
    let module = parse(
        r"
declare float @llvm.fabs.f32(float)

define float @test(float %x) {
  %A = call float @llvm.fabs.f32(float %x)
  ret float %A
}
",
    );
    let function = defined_function(&module);
    let absolute = module
        .as_view()
        .functions()
        .flat_map(|f| f.basic_blocks())
        .flat_map(|block| block.instructions())
        .find(|candidate| candidate.name().as_deref() == Some("A"))
        .expect("fixture defines %A")
        .to_erased();

    let subnormal_normal_infinity = FpClassTest::SUBNORMAL
        .union(FpClassTest::NORMAL)
        .union(FpClassTest::INFINITY);
    let expected: &[(FloatPredicate, FpClassTest)] = &[
        (FloatPredicate::Oeq, FpClassTest::ZERO),
        (
            FloatPredicate::Ueq,
            FpClassTest::ZERO.union(FpClassTest::NAN),
        ),
        (FloatPredicate::Une, FpClassTest::ZERO.complement()),
        (
            FloatPredicate::One,
            FpClassTest::NAN
                .complement()
                .intersection(FpClassTest::ZERO.complement()),
        ),
        (FloatPredicate::Ord, FpClassTest::NAN.complement()),
        (FloatPredicate::Uno, FpClassTest::NAN),
        (FloatPredicate::Ogt, subnormal_normal_infinity),
        (
            FloatPredicate::Ugt,
            subnormal_normal_infinity.union(FpClassTest::NAN),
        ),
        (FloatPredicate::Oge, FpClassTest::NAN.complement()),
        (FloatPredicate::Uge, FpClassTest::ALL),
        (FloatPredicate::Olt, FpClassTest::NONE),
        (FloatPredicate::Ult, FpClassTest::NAN),
        (FloatPredicate::Ole, FpClassTest::ZERO),
        (
            FloatPredicate::Ule,
            FpClassTest::ZERO.union(FpClassTest::NAN),
        ),
    ];

    for (predicate, classes) in expected {
        let implied =
            fcmp_implies_class_of_class(*predicate, function, absolute, FpClassTest::ZERO, true)
                .unwrap_or_else(|| panic!("{predicate:?} implies something"));
        assert_eq!(
            implied.if_true(),
            *classes,
            "classes if true for {predicate:?}"
        );
    }
}

/// Comparing against positive infinity.
///
/// Ports `ComputeKnownFPClassTest.FCmpToClassTest_PInf`
/// (`llvm/unittests/Analysis/ValueTrackingTest.cpp`).
#[test]
fn fcmp_to_class_test_against_positive_infinity() {
    let module = parse(
        r"
define i1 @test(double %arg) {
  %A = fcmp ogt double %arg, 0x7FF0000000000000
  %A2 = fcmp ule double %arg, 0x7FF0000000000000
  %A3 = fcmp ole double %arg, 0x7FF0000000000000
  %A4 = fcmp ugt double %arg, 0x7FF0000000000000
  ret i1 %A
}
",
    );

    // Nothing is ordered and greater than infinity.
    let function = defined_function(&module);
    let (lhs, rhs) = compare_operands(&module, "A");
    assert_eq!(
        fcmp_to_class_test(FloatPredicate::Ogt, function, lhs, rhs, true),
        Some((lhs, FpClassTest::NONE))
    );

    let (lhs, rhs) = compare_operands(&module, "A2");
    assert_eq!(
        fcmp_to_class_test(FloatPredicate::Ule, function, lhs, rhs, true),
        Some((lhs, FpClassTest::ALL))
    );

    // `x <= +inf` is exactly `x` being ordered.
    let (lhs, rhs) = compare_operands(&module, "A3");
    assert_eq!(
        fcmp_to_class_test(FloatPredicate::Ole, function, lhs, rhs, true),
        Some((lhs, FpClassTest::NAN.complement()))
    );

    let (lhs, rhs) = compare_operands(&module, "A4");
    assert_eq!(
        fcmp_to_class_test(FloatPredicate::Ugt, function, lhs, rhs, true),
        Some((lhs, FpClassTest::NAN))
    );
}
