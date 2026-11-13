//! A global named directly by an instruction is a constant operand.
//!
//! `ConstantFoldInstruction` folds when `isa<Constant>` holds of every operand,
//! and that is true of every `GlobalValue`. llvmkit gives each global kind its
//! own storage, so its two dispatchers have to say so: the `DataLayout` one,
//! `constant_folding::constant_fold_instruction`, and the one with no
//! `DataLayout`, `constant_fold::constant_fold_instruction`.
//!
//! Expected answers come from the `CHECK` lines of the fixture each test names.
//! Those fixtures run `instsimplify`; these ask the dispatcher the same question
//! one step in, because `simplifyInstruction` reaches the same folds for a
//! constant operand.

use llvmkit_asmparser::parser;
use llvmkit_ir::{DataLayout, DynBrand, InstructionView, Module, Unverified, constant_folding};

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

/// What the dispatcher with a `DataLayout` folds `%name` of `@function` to.
fn with_layout(
    module: &Module<DynBrand, Unverified>,
    layout: &DataLayout,
    function: &str,
    name: &str,
) -> Option<String> {
    constant_folding::constant_fold_instruction(&named(module, function, name), layout, None)
        .expect("the fold succeeds")
        .map(|folded| folded.to_string())
}

/// What the dispatcher with no `DataLayout` folds `%name` of `@function` to.
fn without_layout(
    module: &Module<DynBrand, Unverified>,
    function: &str,
    name: &str,
) -> Option<String> {
    llvmkit_ir::constant_fold_instruction(&named(module, function, name))
        .expect("the fold succeeds")
        .map(|folded| folded.to_string())
}

/// Ports `test/Transforms/InstSimplify/ConstProp/loads.ll::test6`, whose
/// `CHECK` block ends `ret i64 4607182418800017408`: `@g2` is the load's direct
/// operand, and the fold reaches `ConstantFoldLoadFromConstPtr` only because a
/// `GlobalVariable` is a `Constant`.
#[test]
fn a_load_of_a_global_folds() {
    let module = parse(
        r#"
@g2 = constant double 1.0

define i64 @test6() {
  %r = load i64, ptr @g2
  ret i64 %r
}
"#,
    );
    let layout = module.data_layout().clone();
    assert_eq!(
        with_layout(&module, &layout, "test6", "r").as_deref(),
        Some("i64 4607182418800017408")
    );
}

/// **No upstream counterpart as a fixture**: no `InstSimplify` test asks these
/// of a direct operand. The two guards are upstream's: `LI->isVolatile()` in
/// `ConstantFoldInstOperandsImpl`'s `Load` arm, and `GV->isConstant()` and
/// `GV->hasDefinitiveInitializer()` in `ConstantFoldLoadFromConstPtr`.
/// `%plain` is the positive control: the same load of a global the fold may
/// read.
#[test]
fn a_load_the_fold_may_not_read_stays() {
    let module = parse(
        r#"
@constant = constant i32 7
@variable = global i32 7
@weak = weak constant i32 7

define void @f() {
  %plain = load i32, ptr @constant
  %volatile = load volatile i32, ptr @constant
  %variable = load i32, ptr @variable
  %weak = load i32, ptr @weak
  ret void
}
"#,
    );
    let layout = module.data_layout().clone();
    let fold = |name| with_layout(&module, &layout, "f", name);
    assert_eq!(fold("plain").as_deref(), Some("i32 7"));
    assert_eq!(fold("volatile"), None);
    assert_eq!(fold("variable"), None);
    assert_eq!(fold("weak"), None);
}

/// Ports the comparisons of a function and of a global variable with `null` in
/// `test/Transforms/InstSimplify/ConstProp/icmp-global.ll`, each `(function,
/// expected)` read from that function's `CHECK` block; a `None` is a block that
/// keeps the `icmp`. Both dispatchers answer, because
/// `ConstantFoldCompareInstOperands` ends in `ConstantFoldCompareInstruction`.
#[test]
fn a_global_is_compared_with_null() {
    let module = parse(
        r#"
define i1 @ule_null_constexpr(ptr %x) {
  %cmp = icmp ule ptr null, @ule_null_constexpr
  ret i1 %cmp
}

define i1 @ugt_null_constexpr(ptr %x) {
  %cmp = icmp ugt ptr null, @ugt_null_constexpr
  ret i1 %cmp
}

define i1 @uge_constexpr_null(ptr %x) {
  %cmp = icmp uge ptr @ugt_null_constexpr, null
  ret i1 %cmp
}

define i1 @ult_constexpr_null(ptr %x) {
  %cmp = icmp ult ptr @ugt_null_constexpr, null
  ret i1 %cmp
}

define i1 @ule_constexpr_null(ptr %x) {
  %cmp = icmp ule ptr @ugt_null_constexpr, null
  ret i1 %cmp
}

define i1 @slt_constexpr_null(ptr %x) {
  %cmp = icmp slt ptr @ugt_null_constexpr, null
  ret i1 %cmp
}

@g = global [2 x i32] [i32 1, i32 2]

define i1 @global_ne_null() {
  %cmp = icmp ne ptr @g, null
  ret i1 %cmp
}

define i1 @global_ugt_null() {
  %cmp = icmp ugt ptr @g, null
  ret i1 %cmp
}

define i1 @global_sgt_null() {
  %cmp = icmp sgt ptr @g, null
  ret i1 %cmp
}
"#,
    );
    let layout = module.data_layout().clone();
    for (function, expected) in [
        ("ule_null_constexpr", Some("i1 true")),
        ("ugt_null_constexpr", Some("i1 false")),
        ("uge_constexpr_null", Some("i1 true")),
        ("ult_constexpr_null", Some("i1 false")),
        ("ule_constexpr_null", Some("i1 false")),
        ("slt_constexpr_null", None),
        ("global_ne_null", Some("i1 true")),
        ("global_ugt_null", Some("i1 true")),
        ("global_sgt_null", None),
    ] {
        assert_eq!(
            without_layout(&module, function, "cmp").as_deref(),
            expected,
            "no DataLayout, @{function}"
        );
        assert_eq!(
            with_layout(&module, &layout, function, "cmp").as_deref(),
            expected,
            "DataLayout, @{function}"
        );
    }
}

/// Ports the `getelementptr` of `test/Transforms/InstSimplify/compare.ll::PR31262`:
/// `ConstantFoldGetElementPtr`'s `IsNoOp` counts an `undef` index, so the
/// instruction is `@a`. Its `common` linkage does not matter here.
#[test]
fn a_getelementptr_of_a_global_with_no_offset_is_the_global() {
    let module = parse(
        r#"
@a = common global [1 x i32] zeroinitializer, align 4

define void @PR31262() {
  %idx = getelementptr inbounds [1 x i32], ptr @a, i64 0, i64 undef
  ret void
}
"#,
    );
    assert_eq!(
        without_layout(&module, "PR31262", "idx").as_deref(),
        Some("ptr @a")
    );
}

/// Ports `test/Transforms/InstSimplify/ConstProp/bitcast.ll::addrspacecast_to_gep`,
/// whose `CHECK` block keeps the cast as a constant expression:
/// `ConstantFoldCastOperand` builds one because `isDesirableCastOp` says so.
/// The dispatcher with no `DataLayout` is `ConstantFoldCastInstruction` alone,
/// which has no fold for it.
#[test]
fn an_address_space_cast_of_a_global_is_a_constant_expression() {
    let module = parse(
        r#"
@G = global i32 0

define void @addrspacecast_to_gep() {
  %p = addrspacecast ptr @G to ptr addrspace(1)
  ret void
}
"#,
    );
    let layout = module.data_layout().clone();
    assert_eq!(
        with_layout(&module, &layout, "addrspacecast_to_gep", "p").as_deref(),
        Some("ptr addrspace(1) addrspacecast (ptr @G to ptr addrspace(1))")
    );
    assert_eq!(without_layout(&module, "addrspacecast_to_gep", "p"), None);
}

/// **No upstream counterpart**: `isa<Constant>` is true of every `GlobalValue`,
/// and each of llvmkit's four storage kinds has to say so. `select` hands its
/// chosen operand back, so the answer names the global it was given. No
/// `InstSimplify` fixture has a `select` of a constant condition over globals.
#[test]
fn each_kind_of_global_is_a_constant_operand() {
    let module = parse(
        r#"
@variable = global i32 0
@alias = alias i32, ptr @variable
@ifunc = ifunc void (), ptr @resolver

define ptr @resolver() {
  ret ptr null
}

define void @f() {
  %variable = select i1 true, ptr @variable, ptr null
  %alias = select i1 true, ptr @alias, ptr null
  %ifunc = select i1 true, ptr @ifunc, ptr null
  %function = select i1 true, ptr @resolver, ptr null
  ret void
}
"#,
    );
    let layout = module.data_layout().clone();
    for name in ["variable", "alias", "ifunc"] {
        let expected = format!("ptr @{name}");
        assert_eq!(
            without_layout(&module, "f", name).as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(
            with_layout(&module, &layout, "f", name).as_deref(),
            Some(expected.as_str())
        );
    }
    assert_eq!(
        without_layout(&module, "f", "function").as_deref(),
        Some("ptr @resolver")
    );
    assert_eq!(
        with_layout(&module, &layout, "f", "function").as_deref(),
        Some("ptr @resolver")
    );
}

/// **No upstream counterpart**: the rest of the dispatcher with no `DataLayout`
/// that a global reaches. `ConstantFoldCastInstruction` on a `bitcast` to the
/// same type, and the two inserts, which keep the global as an element. The
/// `ptrtoint` has no fold in `ConstantFoldCastInstruction` and stays.
#[test]
fn a_global_reaches_the_other_arms_of_the_dispatcher_with_no_layout() {
    let module = parse(
        r#"
@g = global i32 0

define void @f() {
  %bitcast = bitcast ptr @g to ptr
  %vector = insertelement <1 x ptr> poison, ptr @g, i32 0
  %struct = insertvalue { ptr } poison, ptr @g, 0
  %ptrtoint = ptrtoint ptr @g to i64
  ret void
}
"#,
    );
    let fold = |name| without_layout(&module, "f", name);
    assert_eq!(fold("bitcast").as_deref(), Some("ptr @g"));
    assert_eq!(fold("vector").as_deref(), Some("<1 x ptr> <ptr @g>"));
    assert_eq!(fold("struct").as_deref(), Some("{ ptr } { ptr @g }"));
    assert_eq!(fold("ptrtoint"), None);
}

/// **No upstream counterpart**: `ConstantFoldInstOperandsImpl` asks
/// `ConstantExpr::isSupportedGetElementPtr` before anything else, and with a
/// global as a constant operand the dispatcher with no `DataLayout` can reach
/// the zero-index fold with a scalable source. `%fixed` is the positive
/// control.
#[test]
fn a_scalable_source_is_refused_by_the_dispatcher_with_no_layout() {
    let module = parse(
        r#"
@g = global i32 0

define void @f() {
  %scalable = getelementptr <vscale x 4 x i32>, ptr @g, i64 0
  %fixed = getelementptr <4 x i32>, ptr @g, i64 0
  ret void
}
"#,
    );
    assert_eq!(without_layout(&module, "f", "scalable"), None);
    assert_eq!(
        without_layout(&module, "f", "fixed").as_deref(),
        Some("ptr @g")
    );
}
