//! The splat family of `llvm/lib/Analysis/VectorUtils.cpp`.
//!
//! Every case below comes from `llvm/unittests/Analysis/VectorUtilsTest.cpp`,
//! IR inlined verbatim: `TEST_F(BasicTest, getSplatIndex)`, the twenty-six
//! `TEST_F(VectorUtilsTest, isSplatValue_*)` cases and the three
//! `getSplatValue*` cases. Upstream's fixture always names the instruction
//! under test `%A`, and [`instruction_a`] reproduces the lookup its harness
//! does.
//!
//! Two of the fixtures exist to pin behaviour upstream itself marks `FIXME` —
//! `isSplatValue_0u*`, where a single poison mask element makes an otherwise
//! obvious splat fail. They are ported as-is: the point of a ported test is to
//! record what upstream answers, not what it could answer.
//!
//! **`findScalarElement` has no upstream unit test.** `VectorUtilsTest.cpp`
//! covers `getSplatIndex`, `isSplatValue` and `getSplatValue` and stops there,
//! so nothing here checks it and its port rests on the implementation alone.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    DynBrand, Module, ShuffleMaskElem, Unverified, Value, get_splat_value, is_splat_value,
    splat_index,
};

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}\n--- source ---\n{source}"),
    }
}

/// Upstream's harness: find the instruction named `%A` in `@test`.
fn instruction_a<'m>(module: &'m Module<DynBrand, Unverified>) -> Value<'m, DynBrand> {
    module
        .as_view()
        .functions()
        .find(|function| function.name() == "test")
        .expect("@test must have a function named @test")
        .basic_blocks()
        .flat_map(|block| block.instructions())
        .find(|instruction| instruction.name().as_deref() == Some("A"))
        .expect("@test must have an instruction %A")
        .to_erased()
}

/// `EXPECT_TRUE(isSplatValue(A))` and its `EXPECT_FALSE` twin, with upstream's
/// optional `Index` argument.
fn expect_splat(source: &str, index: Option<u32>, expected: bool) {
    let module = parse(source);
    assert_eq!(
        is_splat_value(instruction_a(&module), index),
        expected,
        "isSplatValue(A, {index:?})\n{source}"
    );
}

/// Upstream's mask literals are bare `int`s, negative meaning undefined.
/// `ShuffleMaskElem::from_encoded` is the same decoding `getMaskValue`
/// performs, so the fixture's numbers carry over unchanged.
fn mask(elements: &[i64]) -> Vec<ShuffleMaskElem> {
    elements
        .iter()
        .map(|element| ShuffleMaskElem::from_encoded(*element))
        .collect()
}

/// Ports `TEST_F(BasicTest, getSplatIndex)` — all eight assertions, including
/// the ones whose comments explain that negatives are ignored and that an
/// all-negative mask collapses to the same "no splat" answer as a mask holding
/// two different lanes.
#[test]
fn get_splat_index() {
    assert_eq!(splat_index(&mask(&[0, 0, 0])), Some(0));
    assert_eq!(splat_index(&mask(&[1, 0, 0])), None); // no splat
    assert_eq!(splat_index(&mask(&[0, 1, 1])), None); // no splat
    // array size is independent of splat index
    assert_eq!(splat_index(&mask(&[42, 42, 42])), Some(42));
    assert_eq!(splat_index(&mask(&[42, 42, -1])), Some(42)); // ignore negative
    assert_eq!(splat_index(&mask(&[-1, 42, -1])), Some(42)); // ignore negatives
    // ignore all negatives
    assert_eq!(splat_index(&mask(&[-4, 42, -42])), Some(42));
    // all negative values map to "no splat"
    assert_eq!(splat_index(&mask(&[-4, -1, -42])), None);
}

const SHUFFLE_00: &str = r"
define <2 x i8> @test(<2 x i8> %x) {
  %A = shufflevector <2 x i8> %x, <2 x i8> undef, <2 x i32> zeroinitializer
  ret <2 x i8> %A
}
";

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_00)`.
#[test]
fn is_splat_value_00() {
    expect_splat(SHUFFLE_00, None, true);
}

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_00_index0)`.
#[test]
fn is_splat_value_00_index0() {
    expect_splat(SHUFFLE_00, Some(0), true);
}

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_00_index1)`.
#[test]
fn is_splat_value_00_index1() {
    expect_splat(SHUFFLE_00, Some(1), false);
}

const SHUFFLE_11: &str = r"
define <2 x i8> @test(<2 x i8> %x) {
  %A = shufflevector <2 x i8> %x, <2 x i8> undef, <2 x i32> <i32 1, i32 1>
  ret <2 x i8> %A
}
";

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_11)`.
#[test]
fn is_splat_value_11() {
    expect_splat(SHUFFLE_11, None, true);
}

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_11_index0)`.
#[test]
fn is_splat_value_11_index0() {
    expect_splat(SHUFFLE_11, Some(0), false);
}

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_11_index1)`.
#[test]
fn is_splat_value_11_index1() {
    expect_splat(SHUFFLE_11, Some(1), true);
}

const SHUFFLE_01: &str = r"
define <2 x i8> @test(<2 x i8> %x) {
  %A = shufflevector <2 x i8> %x, <2 x i8> undef, <2 x i32> <i32 0, i32 1>
  ret <2 x i8> %A
}
";

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_01)`.
#[test]
fn is_splat_value_01() {
    expect_splat(SHUFFLE_01, None, false);
}

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_01_index0)`.
#[test]
fn is_splat_value_01_index0() {
    expect_splat(SHUFFLE_01, Some(0), false);
}

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_01_index1)`.
#[test]
fn is_splat_value_01_index1() {
    expect_splat(SHUFFLE_01, Some(1), false);
}

const SHUFFLE_0U: &str = r"
define <2 x i8> @test(<2 x i8> %x) {
  %A = shufflevector <2 x i8> %x, <2 x i8> undef, <2 x i32> <i32 0, i32 undef>
  ret <2 x i8> %A
}
";

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_0u)`, which upstream prefaces
/// with `// FIXME: Allow undef matching with Constant (mask) splat analysis.`
/// A single undefined mask element makes `all_equal` fail, so this reads as
/// "not a splat" even though every defined lane agrees.
#[test]
fn is_splat_value_0u() {
    expect_splat(SHUFFLE_0U, None, false);
}

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_0u_index0)`, carrying the same
/// upstream `FIXME`.
#[test]
fn is_splat_value_0u_index0() {
    expect_splat(SHUFFLE_0U, Some(0), false);
}

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_0u_index1)`.
#[test]
fn is_splat_value_0u_index1() {
    expect_splat(SHUFFLE_0U, Some(1), false);
}

const BINOP: &str = r"
define <2 x i8> @test(<2 x i8> %x) {
  %v0 = shufflevector <2 x i8> %x, <2 x i8> undef, <2 x i32> <i32 0, i32 0>
  %v1 = shufflevector <2 x i8> %x, <2 x i8> undef, <2 x i32> <i32 1, i32 1>
  %A = udiv <2 x i8> %v0, %v1
  ret <2 x i8> %A
}
";

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_Binop)`: both operands splat,
/// so the result is one — the case `getSplatValue` cannot see, because there
/// is no single scalar to name.
#[test]
fn is_splat_value_binop() {
    expect_splat(BINOP, None, true);
}

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_Binop_index0)`: `%v1` splats
/// lane 1, so lane 0 is not the defined one.
#[test]
fn is_splat_value_binop_index0() {
    expect_splat(BINOP, Some(0), false);
}

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_Binop_index1)`.
#[test]
fn is_splat_value_binop_index1() {
    expect_splat(BINOP, Some(1), false);
}

const BINOP_CONSTANT_OP0: &str = r"
define <2 x i8> @test(<2 x i8> %x) {
  %v1 = shufflevector <2 x i8> %x, <2 x i8> undef, <2 x i32> <i32 1, i32 1>
  %A = ashr <2 x i8> <i8 42, i8 42>, %v1
  ret <2 x i8> %A
}
";

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_Binop_ConstantOp0)`.
#[test]
fn is_splat_value_binop_constant_op0() {
    expect_splat(BINOP_CONSTANT_OP0, None, true);
}

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_Binop_ConstantOp0_index0)`.
#[test]
fn is_splat_value_binop_constant_op0_index0() {
    expect_splat(BINOP_CONSTANT_OP0, Some(0), false);
}

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_Binop_ConstantOp0_index1)`.
///
/// This is the case that pins upstream's second `FIXME`: the constant operand
/// answers `true` for *any* index, because the constant arm never consults
/// one. Only `%v1` is checked at lane 1, and it matches.
#[test]
fn is_splat_value_binop_constant_op0_index1() {
    expect_splat(BINOP_CONSTANT_OP0, Some(1), true);
}

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_Binop_Not_Op0)`.
#[test]
fn is_splat_value_binop_not_op0() {
    expect_splat(
        r"
define <2 x i8> @test(<2 x i8> %x) {
  %v0 = shufflevector <2 x i8> %x, <2 x i8> undef, <2 x i32> <i32 1, i32 0>
  %v1 = shufflevector <2 x i8> %x, <2 x i8> undef, <2 x i32> <i32 1, i32 1>
  %A = add <2 x i8> %v0, %v1
  ret <2 x i8> %A
}
",
        None,
        false,
    );
}

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_Binop_Not_Op1)`.
#[test]
fn is_splat_value_binop_not_op1() {
    expect_splat(
        r"
define <2 x i8> @test(<2 x i8> %x) {
  %v0 = shufflevector <2 x i8> %x, <2 x i8> undef, <2 x i32> <i32 1, i32 1>
  %v1 = shufflevector <2 x i8> %x, <2 x i8> undef, <2 x i32> <i32 0, i32 1>
  %A = shl <2 x i8> %v0, %v1
  ret <2 x i8> %A
}
",
        None,
        false,
    );
}

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_Select)`: all three operands
/// splat, including the condition.
#[test]
fn is_splat_value_select() {
    expect_splat(
        r"
define <2 x i8> @test(<2 x i1> %x, <2 x i8> %y, <2 x i8> %z) {
  %v0 = shufflevector <2 x i1> %x, <2 x i1> undef, <2 x i32> <i32 1, i32 1>
  %v1 = shufflevector <2 x i8> %y, <2 x i8> undef, <2 x i32> <i32 0, i32 0>
  %v2 = shufflevector <2 x i8> %z, <2 x i8> undef, <2 x i32> <i32 1, i32 1>
  %A = select <2 x i1> %v0, <2 x i8> %v1, <2 x i8> %v2
  ret <2 x i8> %A
}
",
        None,
        true,
    );
}

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_Select_ConstantOp)`.
#[test]
fn is_splat_value_select_constant_op() {
    expect_splat(
        r"
define <2 x i8> @test(<2 x i1> %x, <2 x i8> %y, <2 x i8> %z) {
  %v0 = shufflevector <2 x i1> %x, <2 x i1> undef, <2 x i32> <i32 1, i32 1>
  %v2 = shufflevector <2 x i8> %z, <2 x i8> undef, <2 x i32> <i32 1, i32 1>
  %A = select <2 x i1> %v0, <2 x i8> <i8 42, i8 42>, <2 x i8> %v2
  ret <2 x i8> %A
}
",
        None,
        true,
    );
}

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_Select_NotCond)`: the condition
/// is a bare argument, so nothing is known about it.
#[test]
fn is_splat_value_select_not_cond() {
    expect_splat(
        r"
define <2 x i8> @test(<2 x i1> %x, <2 x i8> %y, <2 x i8> %z) {
  %v1 = shufflevector <2 x i8> %y, <2 x i8> undef, <2 x i32> <i32 0, i32 0>
  %v2 = shufflevector <2 x i8> %z, <2 x i8> undef, <2 x i32> <i32 1, i32 1>
  %A = select <2 x i1> %x, <2 x i8> %v1, <2 x i8> %v2
  ret <2 x i8> %A
}
",
        None,
        false,
    );
}

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_Select_NotOp1)`.
#[test]
fn is_splat_value_select_not_op1() {
    expect_splat(
        r"
define <2 x i8> @test(<2 x i1> %x, <2 x i8> %y, <2 x i8> %z) {
  %v0 = shufflevector <2 x i1> %x, <2 x i1> undef, <2 x i32> <i32 1, i32 1>
  %v2 = shufflevector <2 x i8> %z, <2 x i8> undef, <2 x i32> <i32 1, i32 1>
  %A = select <2 x i1> %v0, <2 x i8> %y, <2 x i8> %v2
  ret <2 x i8> %A
}
",
        None,
        false,
    );
}

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_Select_NotOp2)`.
#[test]
fn is_splat_value_select_not_op2() {
    expect_splat(
        r"
define <2 x i8> @test(<2 x i1> %x, <2 x i8> %y, <2 x i8> %z) {
  %v0 = shufflevector <2 x i1> %x, <2 x i1> undef, <2 x i32> <i32 1, i32 1>
  %v1 = shufflevector <2 x i8> %y, <2 x i8> undef, <2 x i32> <i32 0, i32 0>
  %A = select <2 x i1> %v0, <2 x i8> %v1, <2 x i8> %z
  ret <2 x i8> %A
}
",
        None,
        false,
    );
}

/// Ports `TEST_F(VectorUtilsTest, isSplatValue_SelectBinop)`: the recursion
/// nests — a `select` whose true operand is a binary operator over two splats.
#[test]
fn is_splat_value_select_binop() {
    expect_splat(
        r"
define <2 x i8> @test(<2 x i1> %x, <2 x i8> %y, <2 x i8> %z) {
  %v0 = shufflevector <2 x i1> %x, <2 x i1> undef, <2 x i32> <i32 1, i32 1>
  %v1 = shufflevector <2 x i8> %y, <2 x i8> undef, <2 x i32> <i32 0, i32 0>
  %v2 = shufflevector <2 x i8> %z, <2 x i8> undef, <2 x i32> <i32 1, i32 1>
  %bo = xor <2 x i8> %v1, %v2
  %A = select <2 x i1> %v0, <2 x i8> %bo, <2 x i8> %v2
  ret <2 x i8> %A
}
",
        None,
        true,
    );
}

/// Ports `TEST_F(VectorUtilsTest, getSplatValueElt0)`: the canonical
/// broadcast, whose answer is the *inserted scalar* `%x` rather than the
/// `insertelement` that carries it.
#[test]
fn get_splat_value_elt0() {
    let module = parse(
        r"
define <2 x i8> @test(i8 %x) {
  %ins = insertelement <2 x i8> undef, i8 %x, i32 0
  %A = shufflevector <2 x i8> %ins, <2 x i8> undef, <2 x i32> zeroinitializer
  ret <2 x i8> %A
}
",
    );
    let splat = get_splat_value(instruction_a(&module)).expect("a splat");
    assert_eq!(splat.name().as_deref(), Some("x"));
}

/// Ports `TEST_F(VectorUtilsTest, getSplatValueEltMismatch)`: the scalar goes
/// into lane 1 but the mask reads lane 0, so there is no splat.
#[test]
fn get_splat_value_elt_mismatch() {
    let module = parse(
        r"
define <2 x i8> @test(i8 %x) {
  %ins = insertelement <2 x i8> undef, i8 %x, i32 1
  %A = shufflevector <2 x i8> %ins, <2 x i8> undef, <2 x i32> zeroinitializer
  ret <2 x i8> %A
}
",
    );
    assert!(get_splat_value(instruction_a(&module)).is_none());
}

/// Ports `TEST_F(VectorUtilsTest, getSplatValueElt1)`, which upstream
/// prefaces with `// TODO: This is a splat, but we don't recognize it.`
/// Insert-at-1 broadcast through an all-ones mask is a genuine splat;
/// `m_ZeroMask` only matches lane 0, so it goes unrecognised.
#[test]
fn get_splat_value_elt1() {
    let module = parse(
        r"
define <2 x i8> @test(i8 %x) {
  %ins = insertelement <2 x i8> undef, i8 %x, i32 1
  %A = shufflevector <2 x i8> %ins, <2 x i8> undef, <2 x i32> <i32 1, i32 1>
  ret <2 x i8> %A
}
",
    );
    assert!(get_splat_value(instruction_a(&module)).is_none());
}
