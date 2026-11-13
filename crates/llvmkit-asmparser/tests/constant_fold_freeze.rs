//! A `freeze` of a constant expression, which folds when
//! `isGuaranteedNotToBeUndefOrPoison` proves the expression neither `undef`
//! nor `poison`.
//!
//! The routine reaches a `ConstantExpr` through `if (auto *Opr =
//! dyn_cast<Operator>(V))`: an expression that `canCreateUndefOrPoison(Opr,
//! Kind, true)` says creates neither is well defined when every operand is.
//!
//! The fixtures are upstream's: `test/Transforms/InstSimplify/freeze.ll` and
//! `test/Transforms/SCCP/freeze.ll`. `simplifyFreezeInst` and
//! `SCCPInstVisitor::visitFreezeInst` each ask the routine about the operand
//! as it stands and keep that operand when the answer is yes, which is the
//! question `ConstantFoldInstOperands`'s `case Instruction::Freeze:` asks. So
//! each test hands that arm, through `constant_fold_inst_operands`, the
//! operand its fixture writes, and expects the operand the `CHECK` block
//! returns, or `None` where the block keeps the `freeze`.
//! `simplifyFreezeInst` also passes a context instruction and a dominator
//! tree. They reach only the routine's last two arms, and neither proves a
//! constant here: `programUndefinedIfUndefOrPoison` scans from an instruction
//! or an argument, and no fixture branches on a constant.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    Constant, DataLayout, DynBrand, FoldNonDeterminism, InstructionView, Module, Unverified, User,
    constant_fold_inst_operands, constant_folding,
};

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}\n--- source ---\n{source}"),
    }
}

/// The instruction named `%name` in the function `@function`.
fn named<'m>(
    module: &'m Module<DynBrand, Unverified>,
    function: &str,
    name: &str,
) -> InstructionView<'m, DynBrand> {
    let function = module
        .function_dyn(function)
        .unwrap_or_else(|| panic!("fixture defines @{function}"));
    let found = module
        .view(function)
        .basic_blocks()
        .flat_map(|block| block.instructions())
        .find(|instruction| instruction.name().as_deref() == Some(name));
    found.unwrap_or_else(|| panic!("fixture defines %{name}"))
}

/// The first operand of `%name` in `@function`, as the fixture writes it.
fn written_operand<'m>(
    module: &'m Module<DynBrand, Unverified>,
    function: &str,
    name: &str,
) -> Constant<'m, DynBrand> {
    let operand = named(module, function, name)
        .operand(0)
        .unwrap_or_else(|| panic!("%{name} has an operand"));
    Constant::try_from(operand).unwrap_or_else(|_| panic!("%{name}'s operand is a constant"))
}

/// What `ConstantFoldInstOperands` folds `%name` of `@function` to, handed
/// `operands`.
fn fold<'m>(
    module: &'m Module<DynBrand, Unverified>,
    layout: &DataLayout,
    function: &str,
    name: &str,
    operands: &[Constant<'m, DynBrand>],
) -> Option<Constant<'m, DynBrand>> {
    constant_fold_inst_operands(
        &named(module, function, name),
        operands,
        layout,
        None,
        FoldNonDeterminism::Allow,
    )
    .expect("the fold succeeds")
}

/// Ports the functions of `test/Transforms/InstSimplify/freeze.ll` that freeze
/// a constant expression, or the global beside them: `@constant_expr`,
/// `@constant_expr2`, `@constant_expr3`, `@ptrdiff` and the negative
/// `@vector_element_constant_expr`. The IR is unchanged, and each expected
/// answer is the operand of the `ret` in that function's `CHECK` block, or
/// `None` where the block keeps the `freeze`.
///
/// `@ptrdiff` reaches its `freeze` through three instructions, which
/// `instsimplify` folds first, in order, replacing each with the constant it
/// folds to. They are folded here in that order, each handed the constants
/// before it. `@vector_element_constant_expr` keeps its `freeze` because the
/// routine's constant arm answers a vector that is no expression itself, and
/// refuses one with an expression lane (`containsConstantExpression`).
///
/// `@g` is a global variable, which is a `Constant` upstream. llvmkit stores it
/// as a global, so `@constant_expr2` hands the fold its constant pointer, the
/// operand `constant_fold_instruction` reads for it.
#[test]
fn inst_simplify_folds_a_freeze_of_a_well_defined_constant_expression() {
    let module = parse(
        r#"
@glb = constant i32 0

@g = external global i16, align 1
@g2 = external global i16, align 1

define float @constant_expr() {
  %r = freeze float bitcast (i32 ptrtoint (ptr @g to i32) to float)
  ret float %r
}

define ptr @constant_expr2() {
  %r = freeze ptr @g
  ret ptr %r
}

define ptr @constant_expr3() {
  %r = freeze ptr getelementptr (i32, ptr @glb, i64 3)
  ret ptr %r
}

define i64 @ptrdiff() {
  %i = ptrtoint ptr @g to i64
  %i2 = ptrtoint ptr @g2 to i64
  %diff = sub i64 %i, %i2
  %r = freeze i64 %diff
  ret i64 %r
}

define <2 x i31> @vector_element_constant_expr() {
  %r = freeze <2 x i31> <i31 34, i31 ptrtoint (ptr @g to i31)>
  ret <2 x i31> %r
}
"#,
    );
    let layout = module.data_layout().clone();
    let freeze_of_written = |function: &str| {
        let operand = written_operand(&module, function, "r");
        fold(&module, &layout, function, "r", &[operand]).map(|folded| folded.to_string())
    };

    assert_eq!(
        freeze_of_written("constant_expr").as_deref(),
        Some("float bitcast (i32 ptrtoint (ptr @g to i32) to float)")
    );

    let g = module
        .global("g")
        .expect("fixture defines @g as a global variable");
    let global = module.view(g).as_global_constant_ptr();
    assert_eq!(
        fold(&module, &layout, "constant_expr2", "r", &[global])
            .map(|folded| folded.to_string())
            .as_deref(),
        Some("ptr @g")
    );

    assert_eq!(
        freeze_of_written("constant_expr3").as_deref(),
        Some("ptr getelementptr (i32, ptr @glb, i64 3)")
    );

    let cast = |name: &str| {
        constant_folding::constant_fold_instruction(&named(&module, "ptrdiff", name), &layout, None)
            .expect("the fold succeeds")
            .unwrap_or_else(|| panic!("%{name} casts a global, so it folds"))
    };
    let difference = fold(
        &module,
        &layout,
        "ptrdiff",
        "diff",
        &[cast("i"), cast("i2")],
    )
    .expect("a `sub` of two casts folds to an expression");
    assert_eq!(
        fold(&module, &layout, "ptrdiff", "r", &[difference])
            .map(|folded| folded.to_string())
            .as_deref(),
        Some("i64 sub (i64 ptrtoint (ptr @g to i64), i64 ptrtoint (ptr @g2 to i64))")
    );

    assert_eq!(freeze_of_written("vector_element_constant_expr"), None);
}

/// Ports `test/Transforms/SCCP/freeze.ll::maybe_poison`, IR unchanged, whose
/// `CHECK` block keeps the `freeze`: the fixture's own comment is "make sure we
/// don't constant-propagate values that could potentially be poison". `nuw` is
/// a poison-generating flag (`Operator::hasPoisonGeneratingFlags`), so
/// `canCreateUndefOrPoison` answers that the `add` can create poison and the
/// routine does not look at its operands.
#[test]
fn sccp_keeps_a_freeze_of_an_expression_that_can_create_poison() {
    let module = parse(
        r#"
@g = global i64 0

define i64 @maybe_poison() {
  %fr = freeze i64 add nuw (i64 ptrtoint (ptr @g to i64), i64 123)
  ret i64 %fr
}
"#,
    );
    let layout = module.data_layout().clone();
    let operand = written_operand(&module, "maybe_poison", "fr");
    assert_eq!(
        operand.to_string(),
        "i64 add nuw (i64 ptrtoint (ptr @g to i64), i64 123)",
        "the parser keeps the expression and its flag"
    );
    assert_eq!(
        fold(&module, &layout, "maybe_poison", "fr", &[operand]),
        None
    );
}
